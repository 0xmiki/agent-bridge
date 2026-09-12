import assert from "node:assert/strict";
import {resolve} from "node:path";
import {BridgeHost,type StoredRecord,type RunOptions} from "agent-bridge";
import {readReceipt} from "agent-bridge/receipts";

const [database,workspace,executable,...args] = process.argv.slice(2);
if(!database || !workspace || !executable) throw new Error("bun host/interactions-example.ts <database> <workspace> <ACP executable> [args...]");
const binary = process.env.AGENT_BRIDGE_HOST ?? resolve(import.meta.dir,"../target/debug/agent-bridge-host");
const host = new BridgeHost(binary);
const token = crypto.randomUUID();
const result: NonNullable<RunOptions["result"]> = {name:"selected-token",revision:"v1",mode:"validate_returned_text",max_validation_bytes:4096,
  schema:{type:"object",properties:{token:{type:"string"}},required:["token"],additionalProperties:false}};
let sessionId = ""; let saved: StoredRecord[] = [];
try {
  const codex = executable.includes("codex-acp") || args.some(arg=>arg.includes("codex-acp")) || process.env.AGENT_BRIDGE_CODEX_TEST==="1";
  const session = await host.createSession({database:resolve(database),workspace:resolve(workspace),executable,args,delete_session_on_close:codex});
  sessionId = session.id;
  let records: string[] = [];
  for(let turn=0;turn<2;turn++) {
    const run = session.run("Return the verification token from the selected context as JSON. Do not use tools.", {result,
      context:{mode:"append_to_native",records,resources:turn===0 ? [{id:"verification",revision:"v1",media_type:"text/plain",text:`The verification token is ${token}.`}] : []},
    });
    for await(const event of run.events) if(event.event==="permission") await run.respond(event.permission_id,null);
    const finish = await run.completed;
    assert.equal(finish.status,"completed"); assert.equal(finish.recording_error,null);
    assert.deepEqual(finish.result,{status:"valid",value:{token}});
    saved = (await session.history()).records;
    records = saved.filter(record=>record.run_id===finish.run_id && record.actor==="assistant" && record.payload.type==="message" && (record.payload.data as {kind:string}).kind==="agent").map(record=>record.id);
    assert.equal(records.length,1);
  }
  const receipts = saved.map(readReceipt);
  assert.equal(receipts.filter(r=>r?.kind==="input" && r.data.state==="prepared").length,2);
  assert.equal(receipts.filter(r=>r?.kind==="result_validation" && r.data.validation.status==="valid").length,2);
  console.log("Verified text resource, selected history, and validated results");
} finally { await host.close(); }
const reopened = new BridgeHost(binary);
try { assert.deepEqual((await reopened.history(resolve(database),sessionId)).records,saved); console.log("Verified exact reopened records:",saved.length); }
finally { await reopened.close(); }
