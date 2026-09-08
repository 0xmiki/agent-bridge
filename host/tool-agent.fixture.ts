// Minimal ACP test peer that uses a real MCP stdio client/helper. No AI CLI.
import {writeFileSync} from "node:fs";

if (process.env.BRIDGE_TEST_PID) writeFileSync(process.env.BRIDGE_TEST_PID,String(process.pid));
const emit = (value:unknown) => process.stdout.write(JSON.stringify(value)+"\n");
const reply = (id:unknown,result:unknown) => emit({jsonrpc:"2.0",id,result});
const update = (value:unknown) => emit({jsonrpc:"2.0",method:"session/update",params:{sessionId:"native-tools",update:value}});
class Mcp {
  process: ReturnType<typeof Bun.spawn<"pipe","pipe","inherit">>;
  pending = new Map<number,{resolve:(value:any)=>void;reject:(error:Error)=>void}>();
  next = 0;
  reading:Promise<void>;
  constructor(config:any) {
    this.process = Bun.spawn([config.command,...config.args],{stdin:"pipe",stdout:"pipe",stderr:"inherit",env:{...process.env,...Object.fromEntries(config.env.map((e:any)=>[e.name,e.value]))}});
    if (process.env.BRIDGE_TEST_HELPER_PID) writeFileSync(process.env.BRIDGE_TEST_HELPER_PID,String(this.process.pid));
    const child = this.process;
    this.reading = (async () => {
      let buffer = ""; const decoder = new TextDecoder();
      for await (const bytes of child.stdout) {
        buffer += decoder.decode(bytes,{stream:true}); const lines = buffer.split("\n"); buffer = lines.pop()!;
        for (const line of lines) {
          const frame = JSON.parse(line); const pending = this.pending.get(frame.id);
          if (pending) { this.pending.delete(frame.id); if (frame.error) pending.reject(new Error(JSON.stringify(frame.error))); else pending.resolve(frame.result); }
        }
      }
      for (const pending of this.pending.values()) pending.reject(new Error("MCP disconnected"));
      this.pending.clear();
    })();
    this.reading.catch(error=>{for(const pending of this.pending.values()) pending.reject(error);});
  }
  request(method:string,params:unknown) {
    return new Promise<any>((resolve,reject)=>{
      const id = ++this.next; this.pending.set(id,{resolve,reject});
      this.process.stdin.write(JSON.stringify({jsonrpc:"2.0",id,method,params})+"\n"); this.process.stdin.flush();
    });
  }
  async initialize() {
    await this.request("initialize",{protocolVersion:"2025-03-26",capabilities:{},clientInfo:{name:"bridge-fixture",version:"1"}});
    this.process.stdin.write(JSON.stringify({jsonrpc:"2.0",method:"notifications/initialized"})+"\n"); await this.process.stdin.flush();
  }
  async close() { this.process.stdin.end(); const timer = setTimeout(()=>this.process.kill(),2000); await this.process.exited; clearTimeout(timer); await this.reading.catch(()=>{}); }
}
let mcp:Mcp|undefined;
let active:{id:unknown;cancelled:boolean}|undefined;
async function handle(frame:any) {
  switch(frame.method) {
    case "initialize": reply(frame.id,{protocolVersion:1,agentCapabilities:{},agentInfo:{name:"tool-fixture",version:"1"},authMethods:[]}); break;
    case "session/new": {
      const config = frame.params.mcpServers[0];
      if (config) {
        if (process.env.BRIDGE_TEST_MCP_CONFIG) writeFileSync(process.env.BRIDGE_TEST_MCP_CONFIG,JSON.stringify(config));
        mcp = new Mcp(config); await mcp.initialize();
        const catalog = await mcp.request("tools/list",{});
        if (process.env.BRIDGE_TEST_TOOL_CATALOG) writeFileSync(process.env.BRIDGE_TEST_TOOL_CATALOG,JSON.stringify(catalog));
        if (process.env.BRIDGE_TEST_EARLY_CALL) {
          const early = await mcp.request("tools/call",{name:"project_lookup",arguments:{key:"project"}});
          writeFileSync(process.env.BRIDGE_TEST_EARLY_CALL,JSON.stringify(early));
        }
      }
      reply(frame.id,{sessionId:"native-tools"}); break;
    }
    case "session/prompt": {
      const prompt = {id:frame.id,cancelled:false}; active = prompt;
      if (!mcp) { update({sessionUpdate:"agent_message_chunk",content:{type:"text",text:"no application tools"}}); reply(frame.id,{stopReason:"end_turn"}); break; }
      const name = process.env.BRIDGE_TEST_TOOL_NAME ?? "project_lookup";
      const input = JSON.parse(process.env.BRIDGE_TEST_TOOL_INPUT ?? '{"key":"project"}');
      update({sessionUpdate:"tool_call",toolCallId:"provider-tool",title:name,status:"pending",kind:"other",rawInput:input});
      const invoke = async () => {
        const result = await mcp!.request("tools/call",{name,arguments:input,_meta:{session_id:"spoofed",slot_id:"spoofed",revision:"spoofed"}});
        if (result.isError && process.env.BRIDGE_TEST_CAPACITY_READY) writeFileSync(process.env.BRIDGE_TEST_CAPACITY_READY,"ready");
        return result;
      };
      const result = process.env.BRIDGE_TEST_PARALLEL_TOOLS ? {content:[{type:"text",text:JSON.stringify(await Promise.all(Array.from({length:5},invoke)))}]} : await invoke();
      if (result.isError && process.env.BRIDGE_TEST_RETRY_RESULT) {
        const retry = await mcp.request("tools/call",{name,arguments:input});
        writeFileSync(process.env.BRIDGE_TEST_RETRY_RESULT,JSON.stringify(retry));
      }
      if (process.env.BRIDGE_TEST_MCP_RESULT) writeFileSync(process.env.BRIDGE_TEST_MCP_RESULT,JSON.stringify(result));
      if (!prompt.cancelled) {
        update({sessionUpdate:"tool_call_update",toolCallId:"provider-tool",status:result.isError ? "failed":"completed",rawOutput:result});
        update({sessionUpdate:"agent_message_chunk",messageId:"answer",content:{type:"text",text:result.content?.[0]?.text ?? "no result"}});
        reply(frame.id,{stopReason:"end_turn"});
      }
      break;
    }
    case "session/cancel": if(active && !active.cancelled) {active.cancelled=true;reply(active.id,{stopReason:"cancelled"});} break;
    case "session/delete": reply(frame.id,{}); break;
    default: if(frame.id !== undefined) emit({jsonrpc:"2.0",id:frame.id,error:{code:-32601,message:"unsupported fixture method"}});
  }
}
try {
  let buffer = ""; const decoder = new TextDecoder();
  for await (const bytes of Bun.stdin.stream()) {
    buffer += decoder.decode(bytes,{stream:true}); const lines = buffer.split("\n"); buffer=lines.pop()!;
    for (const line of lines) {
      const frame = JSON.parse(line);
      void handle(frame).catch(error=>emit({jsonrpc:"2.0",id:frame.id,error:{code:-32000,message:String(error)}}));
    }
  }
} finally { await mcp?.close(); }
