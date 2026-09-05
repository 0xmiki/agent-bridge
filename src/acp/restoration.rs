use super::{
    AcpConnection, AcpSession, ContextMode, McpServer, RecordActors, RecordedRun, RecordingError,
};
use crate::context::{ContextLimits, PreparedContext, ResourceStore};
use crate::records::{ContinuationStore, RecordStore};
use crate::{ContextManifest, ContinuationId, RunId, SessionConfiguration, SessionId, SlotId};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PortableRestore {
    pub session_id: SessionId,
    pub slot_id: SlotId,
    pub cwd: PathBuf,
    pub manifest: ContextManifest,
    pub limits: ContextLimits,
    pub max_prompt_bytes: usize,
    pub mode: ContextMode,
}

/// These are exclusive choices. Native errors never trigger a portable fallback.
#[derive(Debug, Clone)]
pub enum RestorationPolicy {
    Native { continuation: ContinuationId },
    Portable(PortableRestore),
}

struct PendingContext {
    context: PreparedContext,
    mode: ContextMode,
    max_prompt_bytes: usize,
}

pub(super) struct PreparedTask<'a> {
    pub prompt: &'a str,
    pub context: &'a PreparedContext,
    pub mode: ContextMode,
    pub max_prompt_bytes: usize,
    pub restoration: Option<Value>,
}

/// A restored session whose first portable turn must use its frozen selection.
pub struct RestoredSession<'connection> {
    session: AcpSession<'connection>,
    pending: Option<PendingContext>,
    report: Value,
    report_pending: bool,
}

impl AcpConnection {
    /// Native resume claims its saved handle. Portable setup resolves all inputs
    /// and validates the encoding before creating a new native session. Neither
    /// choice sends a prompt until start_recorded_run is called on the result.
    pub async fn restore<S: ContinuationStore, R: ResourceStore>(
        &self,
        policy: RestorationPolicy,
        store: &S,
        resources: &R,
        mcp: Vec<McpServer>,
    ) -> Result<RestoredSession<'_>, RecordingError> {
        match policy {
            RestorationPolicy::Native { continuation } => {
                let session = self.resume_saved(store, &continuation, mcp).await?;
                Ok(RestoredSession {
                    session,
                    pending: None,
                    report_pending: true,
                    report: json!({"version":1,"strategy":"native_resume","continuation":continuation.as_str(),
                        "native_context":"reused_uninspected","portable_context_replayed":false}),
                })
            }
            RestorationPolicy::Portable(plan) => {
                let context = crate::context::prepare(
                    &plan.manifest,
                    store,
                    resources,
                    std::slice::from_ref(&plan.session_id),
                    plan.limits,
                )?;
                // Validate unsupported roles/content/capabilities before session/new.
                // The actual task's encoded size is checked again at run dispatch.
                super::context::encode(
                    &context,
                    "",
                    plan.max_prompt_bytes,
                    matches!(plan.mode, ContextMode::AppendImagesToNative),
                    self.info().agent_capabilities.prompt_capabilities.image,
                )?;
                let selected_records: Vec<_> = context
                    .records
                    .iter()
                    .map(|r| json!({"id":r.record.id.as_str(),"revision":r.revision}))
                    .collect();
                let selected_resources: Vec<_> = context
                    .resources
                    .iter()
                    .map(|r| json!({"id":r.reference.id.as_str(),"revision":r.reference.revision}))
                    .collect();
                let selected_instructions: Vec<_> = context.instructions.iter().map(|instruction| json!({
                    "id":instruction.reference.resource.id.as_str(), "revision":instruction.reference.resource.revision,
                    "role":"supplemental", "delivery":"user_text"
                })).collect();
                let report = json!({"version":1,"strategy":"portable_selection","native_context":"new_session",
                    "session_id":plan.session_id.as_str(),"slot_id":plan.slot_id.as_str(),
                    "selected_records":selected_records,"selected_resources":selected_resources,"selected_instructions":selected_instructions,
                    "not_transferred":["history_outside_selection","provider_hidden_state","native_instruction_state","provider_configuration","prior_tool_grants","skill_activation_state"],
                    "delivery":"pending_first_run"});
                let session = self
                    .new_session(plan.session_id, plan.slot_id, plan.cwd, mcp)
                    .await?;
                Ok(RestoredSession {
                    session,
                    pending: Some(PendingContext {
                        context,
                        mode: plan.mode,
                        max_prompt_bytes: plan.max_prompt_bytes,
                    }),
                    report,
                    report_pending: true,
                })
            }
        }
    }
}

impl<'connection> RestoredSession<'connection> {
    /// Setup report, not evidence of input delivery. First-run receipts track that.
    pub fn report(&self) -> &Value {
        &self.report
    }
    pub fn configuration(&self) -> SessionConfiguration {
        self.session.configuration()
    }
    pub async fn set_model(
        &mut self,
        model: impl Into<String>,
    ) -> Result<SessionConfiguration, super::AcpError> {
        self.session.set_model(model).await
    }

    pub async fn set_option(
        &mut self,
        id: &str,
        value: crate::ConfigValue,
    ) -> Result<SessionConfiguration, super::AcpError> {
        self.session.set_option(id, value).await
    }

    pub fn start_recorded_run<'session, 'store, S: RecordStore>(
        &'session mut self,
        id: RunId,
        prompt: impl Into<String>,
        store: &'store S,
        actors: RecordActors,
    ) -> Result<RecordedRun<'session, 'connection, 'store, S>, RecordingError> {
        let prompt = prompt.into();
        let report = self.report_pending.then(|| self.report.clone());
        let result = if let Some(pending) = &self.pending {
            self.session.start_prepared_context_run(
                id,
                PreparedTask {
                    prompt: &prompt,
                    context: &pending.context,
                    mode: pending.mode,
                    max_prompt_bytes: pending.max_prompt_bytes,
                    restoration: report,
                },
                store,
                actors,
            )
        } else {
            self.session
                .start_recorded_with_report(id, prompt, store, actors, report)
        };
        if result.is_ok() {
            self.pending = None;
            self.report_pending = false;
        }
        result
    }

    /// A portable session cannot bypass its first selected-context run.
    pub fn into_session(self) -> Result<AcpSession<'connection>, RecordingError> {
        if self.pending.is_some() {
            return Err(RecordingError::UnsupportedContext(
                "portable context has not been dispatched",
            ));
        }
        Ok(self.session)
    }
}
