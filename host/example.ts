import { BridgeHost, type StoredRecord } from "agent-bridge";
import assert from "node:assert/strict";
import { SessionState } from "agent-bridge/state";
import { resolve } from "node:path";

const [database, workspace, executable, ...args] = process.argv.slice(2);
if (!database || !workspace || !executable) throw new Error("bun host/example.ts <database> <workspace> <ACP executable> [args...]");
const binary = process.env.AGENT_BRIDGE_HOST ?? resolve(import.meta.dir, "../target/debug/agent-bridge-host");
const host = new BridgeHost(binary);
let sessionId = "";
let snapshot: StoredRecord[] = [];
let stateCheckpoint: ReturnType<SessionState["checkpoint"]>;
const phrase = `violet lighthouse ${crypto.randomUUID()}`;
try {
  const codexTest = executable.includes("codex-acp") || args.some(arg => arg.includes("codex-acp")) || process.env.AGENT_BRIDGE_CODEX_TEST === "1";
  const session = await host.createSession({ database: resolve(database), workspace: resolve(workspace), executable, args, delete_session_on_close: codexTest }); sessionId = session.id;
  const state = new SessionState(resolve(database), session.id);
  await state.sync(host);
  assert.equal(state.items.length, 0);
  let turn = 0;
  for (const prompt of [`Remember the phrase ${phrase}. Reply only remembered. Do not use tools.`, "What phrase did I ask you to remember? Reply only the phrase. Do not use tools."]) {
    const run = session.run(prompt);
    let answer = "";
    let terminals = 0;
    for await (const event of run.events) {
      if (event.event === "text_delta") { answer += event.text; process.stdout.write(event.text); }
      if (event.event === "permission") await run.respond(event.permission_id, null);
      if (event.event === "run_finished") terminals++;
    }
    const finish = await run.completed;
    assert.equal(terminals, 1);
    assert.equal(finish.status, "completed");
    assert.equal(finish.recording_error, null);
    console.log("\n", finish.status);
    if (++turn === 2) assert.ok(answer.includes(phrase), "second turn did not preserve the unique phrase");
    const messages = (await session.history()).records.filter(record => record.run_id === finish.run_id && record.payload.type === "message");
    const savedText = (actor: string, kind: string) => messages.filter(record => record.actor === actor && (record.payload.data as { kind: string }).kind === kind).map(record => {
      const data = record.payload.data as { message: { content: { type: string; data: string }[] } };
      return data.message.content.filter(part => part.type === "text").map(part => part.data).join("");
    }).join("");
    assert.equal(savedText("user", "user"), prompt);
    assert.equal(savedText("assistant", "agent"), answer);
  }
  const cancelled = session.run("List the integers from 1 to 1000, one per line. Do not use tools.");
  let requested = false;
  let racedWithCompletion = false;
  let terminals = 0;
  for await (const event of cancelled.events) {
    if (event.event === "text_delta" && !requested) {
      requested = true;
      try { await cancelled.cancel(); }
      catch (error) {
        if (!["not_running", "stale_run"].includes((error as { code: string }).code)) throw error;
        racedWithCompletion = true;
      }
    }
    if (event.event === "permission") await cancelled.respond(event.permission_id, null);
    if (event.event === "run_finished") terminals++;
  }
  const ended = await cancelled.completed;
  assert.ok(requested, "no cancellation was attempted");
  assert.equal(terminals, 1);
  assert.equal(ended.recording_error, null);
  assert.ok(["completed", "cancelled"].includes(ended.status));
  if (racedWithCompletion) assert.equal(ended.status, "completed");
  console.log("Cancellation race outcome:", ended.status);
  snapshot = (await session.history()).records;
  await state.sync(host, 2);
  assert.deepEqual(state.items.map(item => item.record), snapshot);
  stateCheckpoint = state.checkpoint();
  assert.ok(snapshot.every(record => record.session_id === session.id && (
    record.state === "complete" ||
    (record.state === "interrupted" && record.run_id === ended.run_id && ended.status === "cancelled")
  )), "only the cancelled run may contain interrupted records; none may remain open");
  assert.equal(snapshot.filter(record => record.payload.type === "run_finished").length, 3);
} finally { await host.close(); }
const reopened = new BridgeHost(binary);
try {
  const records = (await reopened.history(resolve(database), sessionId)).records;
  assert.deepEqual(records, snapshot);
  const state = new SessionState(resolve(database), sessionId, stateCheckpoint);
  await state.sync(reopened, 2);
  assert.deepEqual(state.items.map(item => item.record), snapshot);
  console.log("Verified reopened records:", records.length);
}
finally { await reopened.close(); }
