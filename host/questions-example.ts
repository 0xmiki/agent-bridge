import assert from "node:assert/strict";
import {resolve} from "node:path";
import {BridgeHost,defineTool,type StoredRecord} from "./client";
import type {Json} from "./tools";

const [database,workspace,executable,...args]=process.argv.slice(2);
if(!database || !workspace || !executable)throw new Error("bun host/questions-example.ts <database> <workspace> <ACP executable> [args...]");
const token=crypto.randomUUID();let shown=0;let resumed=0;
const lookup=defineTool({name:"project_lookup",revision:"v1",description:"Ask the application to choose a project review mode. Call with key project and return verification_token.",
  input:{jsonSchema:{type:"object",properties:{key:{type:"string",enum:["project"]}},required:["key"],additionalProperties:false} as Json,
    parse(value:unknown){if(!value || typeof value!=="object" || (value as {key?:unknown}).key!=="project")throw new Error("key must be project");return value as {key:string};}},
  async execute(_,context){
    const answer=await context.ask({title:"Choose a review mode",fields:[{id:"mode",label:"Mode",required:true,kind:{type:"select",data:{options:[{id:"brief",label:"Brief"},{id:"full",label:"Full"}]}}}]});
    assert.deepEqual(answer,{type:"submitted",data:{mode:{type:"selected",data:"brief"}}});resumed++;
    return {verification_token:token,mode:"brief"};
  },
});
const binary=process.env.AGENT_BRIDGE_HOST ?? resolve(import.meta.dir,"../target/debug/agent-bridge-host");
const host=new BridgeHost(binary,{tools:[lookup],onQuestion:async question=>{
  // Scripted UI input for verification, not a real user's approval.
  assert.equal(question.definition.title,"Choose a review mode");shown++;
  await question.answer({type:"submitted",data:{mode:{type:"selected",data:"brief"}}});
}});
let id="";let records:StoredRecord[]=[];
try{
  const codex=executable.includes("codex-acp") || args.some(arg=>arg.includes("codex-acp")) || process.env.AGENT_BRIDGE_CODEX_TEST==="1";
  const session=await host.createSession({database:resolve(database),workspace:resolve(workspace),executable,args,allowTools:[lookup],delete_session_on_close:codex});id=session.id;
  const run=session.run("Call the project_lookup MCP tool with key project. Reply only with its verification_token. Do not use other tools.");let answer="";
  for await(const event of run.events){if(event.event==="text_delta")answer+=event.text;if(event.event==="permission")await run.respond(event.permission_id,null);}
  const outcome=await run.completed;assert.equal(outcome.status,"completed");assert.equal(outcome.recording_error,null);
  assert.ok(shown>0);assert.equal(shown,resumed);assert.ok(answer.includes(token));
  records=(await session.history()).records;
  const questions=records.filter(row=>row.payload.type==="question");const answers=records.filter(row=>row.payload.type==="answer");
  assert.equal(questions.length,shown);assert.equal(answers.length,shown);
  for(const question of questions){assert.equal(question.state,"complete");assert.ok(answers.some(answer=>answer.reply_to_id===question.id));}
  for(const answer of answers)assert.deepEqual(answer.payload.data,{outcome:{type:"submitted",data:{mode:{type:"selected",data:"brief"}}},delivery:"stored"});
  console.log("Verified scripted questions:",shown,"token:",token);
}finally{await host.close();}
const reopened=new BridgeHost(binary);
try{assert.deepEqual((await reopened.history(resolve(database),id)).records,records);console.log("Verified reopened question records:",records.length);}
finally{await reopened.close();}
