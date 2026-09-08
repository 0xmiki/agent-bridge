import { isDeepStrictEqual } from "node:util";

export type Json = null | boolean | number | string | Json[] | {[key:string]:Json};
export interface ToolReference { name:string; revision:string }
export interface ToolContext {
  readonly invocationId:string; readonly bindingId:string;
  readonly scope: Readonly<{sessionId:string;slotId:string}>;
  readonly signal:AbortSignal;
}
export interface ToolInput<I> { jsonSchema:Json; parse(value:unknown):I }
export interface ApplicationTool extends ToolReference {
  readonly description:string; readonly input_schema:Json;
  /** @internal */ invoke(input:Json,context:ToolContext):Promise<Json>;
}
function check(value:unknown, seen = new Set<object>(), depth = 0): asserts value is Json {
  if (depth > 16) throw new Error("JSON exceeds depth limit");
  if (value === null || typeof value === "string" || typeof value === "boolean") return;
  if (typeof value === "number" && Number.isFinite(value) && (!Number.isInteger(value) || Number.isSafeInteger(value))) return;
  if (typeof value !== "object" || seen.has(value)) throw new Error("value is not safe JSON");
  if (!Array.isArray(value) && Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) throw new Error("value must be plain JSON");
  if (Array.isArray(value) && Object.getOwnPropertyNames(value).length !== value.length + 1) throw new Error("JSON arrays must not have holes or extra properties");
  if (Object.getOwnPropertySymbols(value).length) throw new Error("JSON has symbol keys");
  seen.add(value); for (const item of Object.values(value)) check(item, seen, depth + 1); seen.delete(value);
}
function frozen<T>(value:T):T {
  if (value && typeof value === "object") { Object.values(value).forEach(frozen); Object.freeze(value); }
  return value;
}
export function defineTool<I,O>(options:ToolReference & {description:string; input:ToolInput<I>; execute(input:I,context:ToolContext):O|Promise<O>}):ApplicationTool {
  check(options.input.jsonSchema);
  const schema = frozen(structuredClone(options.input.jsonSchema));
  const adapter = Object.freeze({jsonSchema:schema,parse:options.input.parse});
  const execute = options.execute.bind(Object.freeze({...options,input:adapter}));
  return Object.freeze({name:options.name,revision:options.revision,description:options.description,input_schema:schema,
    async invoke(input:Json,context:ToolContext) {
      const original = frozen(structuredClone(input));
      const parsed = adapter.parse(original);
      if (!isDeepStrictEqual(parsed,original)) throw new Error("tool input parser must not coerce, strip, or add fields");
      const result = await execute(parsed,context);
      check(result);
      if (Buffer.byteLength(JSON.stringify(result)) > 60000) throw new Error("tool result exceeds byte limit");
      return result;
    }});
}

interface Call {event:"tool_call";call_id:string;binding_id:string;session_id:string;slot_id:string;tool:ToolReference;input:Json}
export class ToolClient {
  private definitions = new Map<string,ApplicationTool>();
  private bindings = new Map<string,{session:string;slot:string;tools:ToolReference[]}>();
  private active = new Map<string,{call:Call;controller:AbortController}>();
  private stopped = false;
  constructor(tools:readonly ApplicationTool[],private send:(params:unknown)=>Promise<unknown>) {
    for (const tool of tools) {
      if (this.definitions.has(tool.name)) throw new Error("duplicate tool declaration");
      this.definitions.set(tool.name,tool);
    }
  }
  declarations() { return [...this.definitions.values()].map(({name,revision,description,input_schema})=>({name,revision,description,input_schema})); }
  bind(id:string|null|undefined,session:string,slot:string,tools:ToolReference[]) {
    if (tools.length && typeof id !== "string") throw new Error("missing host tool binding");
    if (id) this.bindings.set(id,{session,slot,tools:structuredClone(tools)});
  }
  stop() { this.stopped = true; for (const entry of this.active.values()) entry.controller.abort("host disconnected"); }
  cancel(frame:Call) {
    const pending = this.active.get(frame.call_id);
    if (!pending) return;
    if (pending.call.binding_id !== frame.binding_id || pending.call.session_id !== frame.session_id || pending.call.slot_id !== frame.slot_id) throw new Error("mismatched tool cancellation");
    pending.controller.abort("tool invocation cancelled; side effects may be uncertain");
  }
  accept(call:Call) {
    if (this.stopped) return;
    const binding = this.bindings.get(call.binding_id);
    const tool = this.definitions.get(call.tool?.name);
    if (!binding || binding.session !== call.session_id || binding.slot !== call.slot_id || !tool || tool.revision !== call.tool.revision || !binding.tools.some(t=>t.name===tool.name && t.revision===tool.revision)
      || typeof call.call_id !== "string" || this.active.has(call.call_id)) throw new Error("invalid application tool invocation");
    const response = (outcome:unknown) => this.send({call_id:call.call_id,binding_id:call.binding_id,session_id:call.session_id,slot_id:call.slot_id,outcome});
    if (this.active.size >= 16) { void response({kind:"error",message:"application handler capacity reached"}).catch(()=>{}); return; }
    const controller = new AbortController();
    this.active.set(call.call_id,{call,controller});
    const context:ToolContext = Object.freeze({invocationId:call.call_id,bindingId:call.binding_id,scope:Object.freeze({sessionId:binding.session,slotId:binding.slot}),signal:controller.signal});
    // Callbacks are independent of run observers. Slots remain occupied until the
    // actual handler returns, even if it ignores its abort signal.
    void Promise.resolve().then(async () => {
      if (controller.signal.aborted) return;
      let outcome:unknown;
      try { outcome = {kind:"success",value:await tool.invoke(call.input,context)}; }
      catch (error) { outcome = {kind:"error",message:String(error).slice(0,2000)}; }
      if (!controller.signal.aborted) await response(outcome);
    }).catch(()=>{}).finally(()=>this.active.delete(call.call_id));
  }
}
