import type { StoredRecord } from "./client";

export interface ResourceRevision { id: string; revision: string }
export interface RecordRevision { id: string; revision: string }
export interface InstructionReference { resource: ResourceRevision; role: "base" | "supplemental" }
export interface ContextManifest { records: string[]; resources: ResourceRevision[]; instructions: InstructionReference[] }
export type ContextItem = {type:"record";id:string} | {type:"resource";resource:ResourceRevision} | {type:"instruction";instruction:InstructionReference};
export interface Omission { item: ContextItem; reason: string }
export interface InstructionAuthority { issuer:string; requester:string; subject:string; granted_instructions:InstructionReference[] }
export type SkillEvidence = {resource:ResourceRevision; native_availability:"unknown"; native_activation:"not_observed"} & (
  {planned_delivery:"require_native"|"supplemental_text";local_availability:"resolved";reason:null} |
  {planned_delivery:"omitted";local_availability:"not_checked";reason:string}
);
export interface ContextPolicyEvidence { omissions:Omission[]; instruction_authority:InstructionAuthority|null; requested_context:ContextManifest; skills:SkillEvidence[] }
export interface ImageEvidence { reference:ResourceRevision;media_type:string;sha256:string;bytes:string;prompt_block:string }
export interface PreparedInput {
  version:1|2|3|4; state:"prepared"; encoding:"agent_bridge.text_context.v1"|"agent_bridge.media_context.v1";
  context_mode:"append_to_native"; wire_text:string; wire_bytes:string; omissions:Omission[];
  images:ImageEvidence[]|null; resource_retention:string|null; instruction_authority:InstructionAuthority|null;
  requested_context:ContextManifest|null; skills:SkillEvidence[]|null;
}
export type InputProgress = {version:1|2|3|4} & ({state:"dispatch_attempted"|"unknown"}|{state:"response_received";stop_reason:string});
export type Restoration = {version:1|2|3} & (
  {strategy:"native_resume";continuation:string;native_context:"reused_uninspected";portable_context_replayed:false} |
  {strategy:"portable_selection";native_context:"new_session";session_id:string;slot_id:string;
   selected_records:RecordRevision[];selected_resources:ResourceRevision[];
   selected_instructions:{id:string;revision:string;role:"supplemental";delivery:"user_text"}[];
   not_transferred:string[];delivery:"pending_first_run";context_policy:ContextPolicyEvidence|null}
);
export interface ContractIdentity { name:string;revision:string }
export interface ResultContract extends ContractIdentity {
  version:1;mode:"validate_returned_text";native_enforcement:false;max_validation_bytes:string;
  validator:"serde_deserialize";application_validation:boolean;wire_text:string;
}
export type Rejection = {kind:"missing_output"|"ambiguous_output"|"non_text_output"|"output_too_large"} |
  {kind:"invalid_json"|"invalid_shape"|"invalid_value"|"incomplete";detail:string};
export interface ResultValidation {
  version:1;contract:ContractIdentity;mode:"validate_returned_text";native_enforcement:false;sources:RecordRevision[];
  validation:{status:"valid"}|{status:"rejected";rejection:Rejection};
}
export type ConfigurationValue = {type:"select";value:string}|{type:"boolean";value:boolean};
type OtherReceipt =
  {kind:"restoration";data:Restoration} |
  {kind:"result_validation";data:ResultValidation} |
  {kind:"configuration_report";data:{confirmed:Record<string,ConfigurationValue>|null}} |
  {kind:"unsupported";data:{name:string;version:number}} |
  {kind:"invalid";data:{name:string;reason:string}};
export type Receipt = OtherReceipt | {kind:"input";data:PreparedInput|InputProgress} | {kind:"result_contract";data:ResultContract};
/** Rust-validated metadata; request text stays once in the original record payload. */
export type ReceiptView = OtherReceipt | {kind:"input";data:Omit<PreparedInput,"wire_text">|InputProgress} | {kind:"result_contract";data:Omit<ResultContract,"wire_text">};

export function readReceipt(record: StoredRecord): Receipt | undefined {
  const receipt = record.receipt;
  if (!receipt) return undefined;
  if (receipt.kind === "result_contract" || (receipt.kind === "input" && receipt.data.state === "prepared")) {
    const wireText = (record.payload.data as {data?:{wire_text?:unknown}})?.data?.wire_text;
    if (typeof wireText !== "string") return {kind:"invalid",data:{name:receipt.kind,reason:"missing request text in receipt payload"}};
    if (receipt.kind === "result_contract") return {kind:receipt.kind,data:{...receipt.data,wire_text:wireText}};
    return {kind:"input",data:{...receipt.data as Omit<PreparedInput,"wire_text">,wire_text:wireText}};
  }
  return receipt as Receipt;
}
