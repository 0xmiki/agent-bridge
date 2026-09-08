import {assertJson} from "./tools";
export type QuestionFieldKind = {type:"text";data:{max_bytes:number}} | {type:"boolean"} |
  {type:"integer";data:{min:number;max:number}} | {type:"select";data:{options:{id:string;label:string}[]}};
export interface QuestionDefinition {title:string;fields:{id:string;label:string;required:boolean;kind:QuestionFieldKind}[]}
export type AnswerValue = {type:"text";data:string}|{type:"boolean";data:boolean}|{type:"integer";data:number}|{type:"selected";data:string};
export type AnswerOutcome = {type:"submitted";data:Record<string,AnswerValue>}|{type:"declined"}|{type:"cancelled"};
export interface QuestionOwner {call_id:string;binding_id:string;session_id:string;slot_id:string}
export interface QuestionView extends QuestionOwner {question_id:string;revision:string;definition:QuestionDefinition}
function freeze<T>(value:T):T {if(value && typeof value==="object"){Object.values(value).forEach(freeze);Object.freeze(value);}return value;}

export class QuestionHandle {
  private controller=new AbortController();
  readonly signal=this.controller.signal;
  error?:Error;
  constructor(readonly view:QuestionView,private request:(method:string,params:unknown)=>Promise<any>) {}
  get id(){return this.view.question_id;}
  get definition(){return this.view.definition;}
  async answer(outcome:AnswerOutcome):Promise<{question_id:string;answer_record_id:string;stored:true}> {
    assertJson(outcome);if(Buffer.byteLength(JSON.stringify(outcome))>60000)throw new Error("answer exceeds byte limit");
    const view=this.view;
    return this.request("question_answer",{question_id:view.question_id,revision:view.revision,call_id:view.call_id,binding_id:view.binding_id,session_id:view.session_id,slot_id:view.slot_id,outcome});
  }
  /** @internal */ close(){this.controller.abort("question closed; inspect stored answer for its outcome");}
}

export class QuestionClient {
  private questions=new Map<string,QuestionHandle>();
  private callbacks=0;
  private stopped=false;
  constructor(private enabled:boolean,private request:(method:string,params:unknown)=>Promise<any>,
    private owns:(owner:QuestionOwner)=>boolean,private onQuestion?: (question:QuestionHandle)=>unknown|Promise<unknown>) {}
  stop(){this.stopped=true;for(const question of this.questions.values())question.close();this.questions.clear();}
  cancelOwner(callId:string){for(const question of this.questions.values()){if(question.view.call_id===callId)question.close();}}
  closed(id:string,owner?:QuestionOwner){
    const handle=this.questions.get(id);
    if(handle && owner && (handle.view.call_id!==owner.call_id || handle.view.binding_id!==owner.binding_id || handle.view.session_id!==owner.session_id || handle.view.slot_id!==owner.slot_id))throw new Error("mismatched question closure");
    handle?.close();
  }
  opened(view:QuestionView,notify=true):QuestionHandle|undefined {
    if(!this.enabled)throw new Error("unnegotiated question event");
    for(const [id,handle] of this.questions){if(!this.owns(handle.view)){handle.close();this.questions.delete(id);}}
    if(this.stopped || !this.owns(view))return;
    const existing=this.questions.get(view.question_id);if(existing)return existing;
    if(typeof view.question_id!=="string" || typeof view.revision!=="string" || !view.definition)throw new Error("invalid question event");
    if(this.questions.size>=128 || [...this.questions.values()].filter(q=>!q.signal.aborted).length>=16)throw new Error("question capacity exceeded");
    const handle=new QuestionHandle(freeze(structuredClone(view)),this.request);
    this.questions.set(view.question_id,handle);
    if(notify && this.onQuestion){
      if(this.callbacks>=16){handle.error=new Error("question UI callback capacity reached");return handle;}
      this.callbacks++;
      void Promise.resolve().then(()=>handle.signal.aborted ? undefined : this.onQuestion!(handle))
        .catch(error=>handle.error=error instanceof Error ? error : new Error(String(error)))
        .finally(()=>this.callbacks--);
    }
    return handle;
  }
  async ask(owner:QuestionOwner,definition:QuestionDefinition,signal:AbortSignal):Promise<AnswerOutcome> {
    if(!this.enabled)throw new Error("question protocol is not enabled");
    if(signal.aborted || this.stopped)throw new Error("tool invocation ended");
    assertJson(definition);if(Buffer.byteLength(JSON.stringify(definition))>16384)throw new Error("question exceeds byte limit");
    const result=await this.request("question_ask",{...owner,definition});
    if(signal.aborted || result.invocation_active!==true)throw new Error("tool invocation ended; any stored answer remains evidence");
    return result.outcome;
  }
  async pending(sessionId?:string):Promise<QuestionHandle[]> {
    if(!this.enabled)throw new Error("question protocol is not enabled");
    const views:QuestionView[]=await this.request("question_pending",{session_id:sessionId});
    return views.map(view=>this.opened(view,false)).filter((value):value is QuestionHandle=>value!==undefined && !value.signal.aborted);
  }
}
