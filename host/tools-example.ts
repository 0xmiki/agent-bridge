import assert from "node:assert/strict";
import {resolve} from "node:path";
import {BridgeHost, defineTool, type StoredRecord} from "./client";
import {readReceipt} from "./receipts";
import type {Json} from "./tools";

const [database,workspace,executable,...args] = process.argv.slice(2);
if (!database || !workspace || !executable) throw new Error("bun host/tools-example.ts <database> <workspace> <ACP executable> [args...]");
const token = crypto.randomUUID(); let calls = 0;
const lookup = defineTool({name:"project_lookup",revision:"v1",description:"Look up the current application project using key project. Returns a verification_token.",
  input:{jsonSchema:{type:"object",properties:{key:{type:"string",enum:["project"]}},required:["key"],additionalProperties:false} as Json,
    parse(value:unknown){if(!value || typeof value!=="object" || (value as {key?:unknown}).key!=="project")throw new Error("key must be project");return value as {key:string};}},
  execute(_,context){calls++;return {project:"agent-bridge",verification_token:token,session:context.scope.sessionId};},
});
const binary = process.env.AGENT_BRIDGE_HOST ?? resolve(import.meta.dir,"../target/debug/agent-bridge-host");
const host = new BridgeHost(binary,{tools:[lookup]});
let id = ""; let records:StoredRecord[] = [];
try {
  const codex = executable.includes("codex-acp") || args.some(arg=>arg.includes("codex-acp")) || process.env.AGENT_BRIDGE_CODEX_TEST === "1";
  const session = await host.createSession({database:resolve(database),workspace:resolve(workspace),executable,args,allowTools:[lookup],delete_session_on_close:codex}); id=session.id;
  const run = session.run("Call the project_lookup MCP tool with key project. Reply only with its verification_token. Do not use other tools.");
  let answer = "";
  for await(const event of run.events){
    if(event.event==="text_delta")answer+=event.text;
    if(event.event==="permission")await run.respond(event.permission_id,null);
  }
  const ended = await run.completed;
  assert.equal(ended.status,"completed");assert.equal(ended.recording_error,null);
  assert.ok(calls>0);assert.ok(answer.includes(token),"agent did not return the application's fresh token");
  records=(await session.history()).records;
  const evidence=records.map(readReceipt).filter(r=>r?.kind==="tool_invocation");
  assert.equal(evidence.filter(r=>r.data.state==="returned").length,calls);
  assert.ok(evidence.every(r=>r.data.scope.session===id));
  assert.ok(!evidence.some(r=>r.data.state==="unknown"));
  console.log("Verified application calls:",calls,"token:",token);
} finally {await host.close();}
const reopened=new BridgeHost(binary);
try {assert.deepEqual((await reopened.history(resolve(database),id)).records,records);console.log("Verified reopened records:",records.length);}
finally {await reopened.close();}
