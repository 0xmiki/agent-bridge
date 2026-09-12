import assert from "node:assert/strict";
import { resolve } from "node:path";
import { BridgeHost, defineTool, type StoredRecord } from "agent-bridge";
import { SessionState } from "agent-bridge/state";
import { readReceipt } from "agent-bridge/receipts";

const [binary, fixture, configFixture] = process.argv.slice(2);
assert.ok(binary && fixture && configFixture, "Provide the host binary and fixture paths");
const database = resolve("consumer.sqlite3");
const token = crypto.randomUUID();
let calls = 0;
let questions = 0;
let cancelled = false;
const waiting = Promise.withResolvers<void>();
const lookup = defineTool({
  name: "project_lookup", revision: "v1", description: "Read the application project",
  input: {
    jsonSchema: { type: "object", properties: { key: { type: "string", enum: ["project"] } }, required: ["key"], additionalProperties: false },
    parse(value: unknown) {
      assert.deepEqual(value, { key: "project" });
      return { key: "project" };
    },
  },
  async execute(_, context) {
    calls++;
    if (calls === 2) {
      await new Promise<void>(resolve => {
        context.signal.addEventListener("abort", () => { cancelled = true; resolve(); }, { once: true });
        waiting.resolve();
      });
      return { cancelled: true };
    }
    const answer = await context.ask({ title: "Review project?", fields: [
      { id: "review", label: "Review", required: true, kind: { type: "boolean" } },
    ] });
    assert.deepEqual(answer, { type: "submitted", data: { review: { type: "boolean", data: true } } });
    return { token };
  },
});
const host = new BridgeHost(resolve(binary), { tools: [lookup], onQuestion: async question => {
  questions++;
  await question.answer({ type: "submitted", data: { review: { type: "boolean", data: true } } });
} });
let sessionId = "";
let records: StoredRecord[] = [];
let continuation="";let continuedSession="";
try {
  const configured = await host.createSession({ database, workspace: process.cwd(), executable: resolve(configFixture), args: ["config"],continuation_scope:"consumer" });
  assert.equal(configured.initialConfiguration?.options?.find(option => option.category === "model")?.choices.length, 2);
  for (const model of ["model-a", "model-b"]) {
    const config = await configured.setModel(model);
    assert.deepEqual(config.values.confirmed?.model, { type: "select", value: model });
    assert.deepEqual(await configured.configuration(), config);
    const turn = configured.run("Say hello");
    for await (const _ of turn.events) { /* Drain the public stream. */ }
    assert.equal((await turn.completed).status, "completed");
  }
  continuedSession=configured.id;continuation=(await configured.handoff()).continuation_id;
  const session = await host.createSession({ database, workspace: process.cwd(), executable: process.execPath,
    args: [resolve(fixture)], allowTools: [lookup] });
  sessionId = session.id;
  const run = session.run("Look up the project", {
    context: {mode:"append_to_native",resources:[{id:"project",revision:"v1",media_type:"text/plain",text:"Use the project_lookup application tool."}]},
    result: {name:"project",revision:"v1",mode:"validate_returned_text",max_validation_bytes:4096,
      schema:{type:"object",properties:{token:{const:token}},required:["token"],additionalProperties:false}},
  });
  let text = "";
  for await (const event of run.events) {
    if (event.event === "text_delta") text += event.text;
    if (event.event === "permission") await run.respond(event.permission_id, null);
  }
  assert.equal((await run.completed).status, "completed");
  assert.equal((await run.completed).recording_error, null);
  assert.deepEqual(JSON.parse(text), { token });
  assert.deepEqual((await run.completed).result, {status:"valid",value:{token}});
  assert.equal(questions, 1);
  const next = session.run("Wait for cancellation");
  next.events.close();
  await waiting.promise;
  await next.cancel();
  assert.equal((await next.completed).status, "cancelled");
  assert.equal((await next.completed).recording_error, null);
  assert.equal(cancelled, true);
  assert.equal(calls, 2);
  records = (await session.history()).records;
  assert.ok(records.some(record => record.payload.type === "answer" && record.state === "complete"));
  assert.ok(records.map(readReceipt).some(receipt => receipt?.kind === "tool_invocation" && receipt.data.state === "returned"));
  assert.ok(records.map(readReceipt).some(receipt => receipt?.kind === "input" && receipt.data.state === "prepared"));
  assert.ok(records.map(readReceipt).some(receipt => receipt?.kind === "result_validation" && receipt.data.validation.status === "valid"));
} finally { await host.close(); }

const reopened = new BridgeHost(resolve(binary));
try {
  assert.deepEqual((await reopened.history(database, sessionId)).records, records);
  const state = new SessionState(database, sessionId);
  await state.sync(reopened);
  assert.equal(state.items.length, records.length);
  assert.ok(state.items.some(item => item.kind === "message" && item.role === "agent" && item.text.includes(token)));
  assert.ok((await reopened.discover(database,"sessions")).items.some(item=>item.id===sessionId));
  assert.ok((await reopened.discover(database,"runs")).items.every(item=>item.dispatch==="attempted"));
  assert.ok((await reopened.discover(database,"continuations")).items.some(item=>item.id===continuation && item.state==="available"));
  const resumed=await reopened.restoreSession({database,workspace:process.cwd(),executable:resolve(configFixture),args:["chat"],continuation_scope:"consumer"},
    {strategy:"native",session_id:continuedSession,continuation});
  const turn=resumed.run("Continue explicitly");for await(const _ of turn.events){}
  assert.equal((await turn.completed).status,"completed");
} finally { await reopened.close(); }
console.log("Packaged consumer passed: configuration, context, validated result, tool, question, cancellation, restart discovery, native resume");
