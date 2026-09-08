//! Version-aware readers for bridge-owned evidence. Parsing is not verification
//! of provider execution, resource retention, instruction authority, or activation.
use super::Payload;
use crate::structured::JsonRejection;
use crate::{
    ActorId, ConfigValues, ContextManifest, ContinuationId, InstructionRef, InstructionRole,
    RecordId, ResourceId, ResourceRef, SessionId, SlotId,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordRevision {
    pub id: RecordId,
    pub revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextItem {
    Record { id: RecordId },
    Resource { resource: ResourceRef },
    Instruction { instruction: InstructionRef },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Omission {
    pub item: ContextItem,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionAuthority {
    pub issuer: ActorId,
    pub requester: ActorId,
    pub subject: ActorId,
    pub granted_instructions: Vec<InstructionRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillEvidence {
    pub resource: ResourceRef,
    pub planned_delivery: String,
    pub local_availability: String,
    pub native_availability: String,
    pub native_activation: String,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPolicyEvidence {
    pub omissions: Vec<Omission>,
    pub instruction_authority: Option<InstructionAuthority>,
    pub requested_context: ContextManifest,
    #[serde(default)]
    pub skills: Vec<SkillEvidence>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageEvidence {
    pub reference: ResourceRef,
    pub media_type: String,
    pub sha256: String,
    pub bytes: u64,
    pub prompt_block: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedInput {
    pub encoding: String,
    pub context_mode: String,
    pub wire_text: String,
    pub wire_bytes: u64,
    pub omissions: Vec<Omission>,
    pub images: Option<Vec<ImageEvidence>>,
    pub resource_retention: Option<String>,
    pub instruction_authority: Option<InstructionAuthority>,
    pub requested_context: Option<ContextManifest>,
    pub skills: Option<Vec<SkillEvidence>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InputStage {
    Prepared(Box<PreparedInput>),
    DispatchAttempted,
    ResponseReceived { stop_reason: String },
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputReceipt {
    pub version: u32,
    #[serde(flatten)]
    pub stage: InputStage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedInstruction {
    pub id: ResourceId,
    pub revision: String,
    pub role: InstructionRole,
    pub delivery: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum RestorationStrategy {
    NativeResume {
        continuation: ContinuationId,
        native_context: String,
        portable_context_replayed: bool,
    },
    PortableSelection {
        native_context: String,
        session_id: SessionId,
        slot_id: SlotId,
        selected_records: Vec<RecordRevision>,
        selected_resources: Vec<ResourceRef>,
        selected_instructions: Vec<SelectedInstruction>,
        not_transferred: Vec<String>,
        delivery: String,
        context_policy: Option<Box<ContextPolicyEvidence>>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorationReceipt {
    pub version: u32,
    #[serde(flatten)]
    pub strategy: RestorationStrategy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractIdentity {
    pub name: String,
    pub revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultContract {
    pub version: u32,
    pub name: String,
    pub revision: String,
    pub mode: String,
    pub native_enforcement: bool,
    pub max_validation_bytes: u64,
    pub validator: String,
    pub application_validation: bool,
    pub wire_text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ValidationOutcome {
    Valid,
    Rejected { rejection: JsonRejection },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultValidation {
    pub version: u32,
    pub contract: ContractIdentity,
    pub mode: String,
    pub native_enforcement: bool,
    pub sources: Vec<RecordRevision>,
    pub validation: ValidationOutcome,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationReport {
    pub confirmed: Option<ConfigValues>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolOutcome {
    Success { value: Value },
    Error { message: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ToolInvocationStage {
    DispatchAttempted { input: Value },
    Returned { outcome: ToolOutcome },
    Unknown { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocationReceipt {
    pub version: u32,
    pub invocation_id: String,
    pub binding_id: String,
    pub scope: crate::ToolScope,
    pub tool: crate::ToolRef,
    pub issuer: ActorId,
    pub subject: ActorId,
    #[serde(flatten)]
    pub stage: ToolInvocationStage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Receipt {
    Input(InputReceipt),
    Restoration(Box<RestorationReceipt>),
    ResultContract(ResultContract),
    ResultValidation(ResultValidation),
    ConfigurationReport(ConfigurationReport),
    ToolInvocation(Box<ToolInvocationReceipt>),
    Unsupported { name: String, version: u32 },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptError {
    pub name: String,
    pub reason: String,
}
impl fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} receipt: {}", self.name, self.reason)
    }
}
impl Error for ReceiptError {}
fn decode<T: DeserializeOwned>(name: &str, data: &Value) -> Result<T, ReceiptError> {
    T::deserialize(data).map_err(|error| ReceiptError {
        name: name.into(),
        reason: error.to_string(),
    })
}
fn require(name: &str, condition: bool, reason: &str) -> Result<(), ReceiptError> {
    if condition {
        Ok(())
    } else {
        Err(ReceiptError {
            name: name.into(),
            reason: reason.into(),
        })
    }
}
fn skills_valid(skills: &[SkillEvidence]) -> bool {
    skills.iter().all(|s| {
        s.native_availability == "unknown"
            && s.native_activation == "not_observed"
            && match s.planned_delivery.as_str() {
                "supplemental_text" | "require_native" => {
                    s.local_availability == "resolved" && s.reason.is_none()
                }
                "omitted" => {
                    s.local_availability == "not_checked"
                        && s.reason.as_ref().is_some_and(|r| !r.trim().is_empty())
                }
                _ => false,
            }
    })
}

/// Unknown namespaces/names return None. Future versions return Unsupported.
/// Known malformed evidence is an error, never a synthesized success or omission.
/// Additive unknown fields remain available in the original payload.
pub fn read(payload: &Payload) -> Result<Option<Receipt>, ReceiptError> {
    let Payload::Extension {
        namespace,
        name,
        data,
    } = payload
    else {
        return Ok(None);
    };
    if namespace != "agent_bridge"
        || ![
            "input_receipt",
            "restoration",
            "result_contract",
            "result_validation",
            "configuration_report",
            "tool_invocation",
        ]
        .contains(&name.as_str())
    {
        return Ok(None);
    }
    require(name, data.is_object(), "expected an object")?;
    if name == "configuration_report" && data.get("version").is_none() {
        require(
            name,
            data.get("confirmed").is_some(),
            "missing confirmed report",
        )?;
        return Ok(Some(Receipt::ConfigurationReport(decode(name, data)?)));
    }
    let version = data["version"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| ReceiptError {
            name: name.clone(),
            reason: "missing or invalid version".into(),
        })?;
    let supported = match name.as_str() {
        "input_receipt" => version <= 4,
        "restoration" => version <= 3,
        "result_contract" | "result_validation" | "tool_invocation" => version == 1,
        _ => false,
    };
    if !supported {
        return Ok(Some(Receipt::Unsupported {
            name: name.clone(),
            version,
        }));
    }
    let receipt = match name.as_str() {
        "tool_invocation" => {
            let value: ToolInvocationReceipt = decode(name, data)?;
            require(
                name,
                !value.invocation_id.trim().is_empty()
                    && !value.binding_id.trim().is_empty()
                    && !value.tool.name.trim().is_empty()
                    && !value.tool.revision.trim().is_empty(),
                "invalid invocation identity",
            )?;
            Receipt::ToolInvocation(Box::new(value))
        }
        "input_receipt" => {
            let value: InputReceipt = decode(name, data)?;
            if let InputStage::Prepared(input) = &value.stage {
                require(
                    name,
                    input.context_mode == "append_to_native",
                    "unsupported context mode",
                )?;
                match input.encoding.as_str() {
                    "agent_bridge.text_context.v1" => require(
                        name,
                        input.images.is_none() && input.wire_bytes == input.wire_text.len() as u64,
                        "inconsistent text encoding or byte count",
                    )?,
                    "agent_bridge.media_context.v1" => require(
                        name,
                        version >= 2
                            && input
                                .images
                                .as_ref()
                                .is_some_and(|images| !images.is_empty())
                            && input.resource_retention.as_deref()
                                == Some("supplied_resource_store")
                            && input.wire_bytes >= input.wire_text.len() as u64,
                        "incomplete image evidence",
                    )?,
                    _ => {
                        return Err(ReceiptError {
                            name: name.clone(),
                            reason: "unsupported input encoding".into(),
                        });
                    }
                }
                if let Some(images) = &input.images {
                    require(
                        name,
                        images.iter().all(|image| {
                            image.sha256.len() == 64
                                && image.sha256.bytes().all(|c| c.is_ascii_hexdigit())
                                && image.prompt_block > 0
                        }),
                        "invalid image descriptor",
                    )?;
                }
                if version == 3 {
                    require(
                        name,
                        data.get("instruction_authority").is_some()
                            && input.requested_context.is_some(),
                        "missing policy evidence",
                    )?;
                }
                require(
                    name,
                    version >= 3
                        || (input.omissions.is_empty()
                            && input.instruction_authority.is_none()
                            && input.requested_context.is_none()),
                    "policy evidence requires version 3 or later",
                )?;
                require(
                    name,
                    (input.omissions.is_empty() && input.instruction_authority.is_none())
                        || input.requested_context.is_some(),
                    "policy evidence is missing requested context",
                )?;
                if version == 4 {
                    require(name, input.skills.is_some(), "missing skill evidence")?;
                }
                require(
                    name,
                    version >= 4 || input.skills.is_none(),
                    "skills require receipt version 4",
                )?;
                require(
                    name,
                    input
                        .skills
                        .as_ref()
                        .is_none_or(|skills| skills_valid(skills)),
                    "inconsistent skill evidence",
                )?;
            }
            Receipt::Input(value)
        }
        "restoration" => {
            let value: RestorationReceipt = decode(name, data)?;
            match &value.strategy {
                RestorationStrategy::NativeResume {
                    native_context,
                    portable_context_replayed,
                    ..
                } => require(
                    name,
                    version == 1
                        && native_context == "reused_uninspected"
                        && !portable_context_replayed,
                    "inconsistent native restoration",
                )?,
                RestorationStrategy::PortableSelection {
                    native_context,
                    delivery,
                    selected_instructions,
                    context_policy,
                    ..
                } => {
                    require(
                        name,
                        native_context == "new_session"
                            && delivery == "pending_first_run"
                            && selected_instructions.iter().all(|i| {
                                i.role == InstructionRole::Supplemental && i.delivery == "user_text"
                            }),
                        "inconsistent portable restoration",
                    )?;
                    require(
                        name,
                        version != 2 || context_policy.is_some(),
                        "missing policy evidence",
                    )?;
                    require(
                        name,
                        version != 1 || context_policy.is_none(),
                        "policy evidence requires restoration version 2 or later",
                    )?;
                    require(
                        name,
                        version >= 3 || context_policy.as_ref().is_none_or(|p| p.skills.is_empty()),
                        "skills require restoration version 3",
                    )?;
                    require(
                        name,
                        context_policy
                            .as_ref()
                            .is_none_or(|p| skills_valid(&p.skills)),
                        "inconsistent skill evidence",
                    )?;
                }
            }
            Receipt::Restoration(Box::new(value))
        }
        "result_contract" => {
            let value: ResultContract = decode(name, data)?;
            require(
                name,
                value.mode == "validate_returned_text"
                    && !value.native_enforcement
                    && value.validator == "serde_deserialize"
                    && value.max_validation_bytes > 0
                    && !value.name.trim().is_empty()
                    && !value.revision.trim().is_empty(),
                "inconsistent result contract",
            )?;
            Receipt::ResultContract(value)
        }
        "result_validation" => {
            let value: ResultValidation = decode(name, data)?;
            require(
                name,
                value.mode == "validate_returned_text"
                    && !value.native_enforcement
                    && !value.contract.name.trim().is_empty()
                    && !value.contract.revision.trim().is_empty(),
                "inconsistent result validation",
            )?;
            require(
                name,
                !matches!(value.validation, ValidationOutcome::Valid)
                    || data["validation"].get("rejection").is_none(),
                "valid outcome contains rejection",
            )?;
            Receipt::ResultValidation(value)
        }
        _ => unreachable!(),
    };
    Ok(Some(receipt))
}
