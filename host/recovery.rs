use super::{
    interactions::{HostedRun, Selection, context_task},
    storage::Store,
};
use agent_bridge::{
    acp::*,
    context::*,
    records::{ContinuationStore, RecordStore},
    structured::JsonContract,
    *,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case", deny_unknown_fields)]
pub enum Restore {
    Native {
        session_id: SessionId,
        continuation: ContinuationId,
    },
    Portable {
        session_id: SessionId,
        context: Selection,
    },
}
impl Restore {
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::Native { session_id, .. } | Self::Portable { session_id, .. } => session_id,
        }
    }
    pub fn prepare(
        self,
        store: &Store,
        workspace: &str,
    ) -> Result<(RestorationPolicy, MemoryResourceStore, SlotId), String> {
        match self {
            Self::Native {
                session_id,
                continuation,
            } => {
                let saved = store
                    .get_continuation(&continuation)
                    .map_err(|e| e.to_string())?;
                if saved.continuation.session_id != session_id
                    || saved.continuation.data["cwd"] != workspace
                {
                    return Err("continuation session/workspace mismatch".into());
                }
                Ok((
                    RestorationPolicy::Native { continuation },
                    MemoryResourceStore::default(),
                    saved.continuation.slot_id.clone(),
                ))
            }
            Self::Portable {
                session_id,
                context,
            } => {
                store
                    .list(&session_id, None, 1)
                    .map_err(|e| e.to_string())?;
                let (manifest, resources) = context.prepare()?;
                let slot_id = SlotId::new(super::identity("slot")).unwrap();
                Ok((
                    RestorationPolicy::portable(PortableRestore {
                        policy: ContextPolicy::default(),
                        session_id,
                        slot_id: slot_id.clone(),
                        cwd: workspace.into(),
                        manifest,
                        limits: ContextLimits {
                            max_items: 128,
                            max_resource_bytes: 262144,
                        },
                        max_prompt_bytes: 524288,
                        mode: ContextMode::AppendToNative,
                    }),
                    resources,
                    slot_id,
                ))
            }
        }
    }
}

// At most eight worker-local sessions; keep ownership inline rather than boxing it.
#[allow(clippy::large_enum_variant)]
pub enum Session<'c> {
    Active(AcpSession<'c>),
    Restored(RestoredSession<'c>),
}
impl<'c> Session<'c> {
    pub fn configuration(&self) -> SessionConfiguration {
        match self {
            Self::Active(s) => s.configuration(),
            Self::Restored(s) => s.configuration(),
        }
    }
    pub async fn set_model(&mut self, model: &str) -> Result<SessionConfiguration, AcpError> {
        match self {
            Self::Active(s) => s.set_model(model).await,
            Self::Restored(s) => s.set_model(model).await,
        }
    }
    pub async fn set_option(
        &mut self,
        id: &str,
        value: ConfigValue,
    ) -> Result<SessionConfiguration, AcpError> {
        match self {
            Self::Active(s) => s.set_option(id, value).await,
            Self::Restored(s) => s.set_option(id, value).await,
        }
    }
    pub fn start<'s, 'd, 'v>(
        &'s mut self,
        id: RunId,
        prompt: &'v str,
        context: Option<&(ContextManifest, MemoryResourceStore)>,
        contract: Option<&'v JsonContract<Value>>,
        store: &'d Store,
        actors: RecordActors,
    ) -> Result<HostedRun<'s, 'c, 'd, 'v>, RecordingError> {
        let session = match self {
            Self::Restored(session) => {
                if context.is_some() || contract.is_some() {
                    return Err(RecordingError::UnsupportedContext(
                        "first restored run must use its frozen selection without run options",
                    ));
                }
                return session
                    .start_recorded_run(id, prompt, store, actors)
                    .map(HostedRun::Plain);
            }
            Self::Active(session) => session,
        };
        if let Some(contract) = contract {
            let task = JsonTask {
                prompt,
                contract,
                mode: JsonOutputMode::ValidateReturnedText,
            };
            if let Some((manifest, resources)) = context {
                session.start_recorded_context_json_run(
                    id,
                    task,
                    context_task(prompt, manifest, resources),
                    store,
                    actors,
                )
            } else {
                session.start_recorded_json_run(id, task, store, actors)
            }
            .map(HostedRun::Json)
        } else {
            if let Some((manifest, resources)) = context {
                session.start_recorded_context_run(
                    id,
                    context_task(prompt, manifest, resources),
                    store,
                    actors,
                )
            } else {
                session.start_recorded_run(id, prompt, store, actors)
            }
            .map(HostedRun::Plain)
        }
    }
    /// Called only after a successfully dispatched first run has been dropped.
    pub fn into_active(self) -> Result<Self, RecordingError> {
        match self {
            Self::Restored(s) => Ok(Self::Active(s.into_session()?)),
            active => Ok(active),
        }
    }
    pub async fn delete(self) -> Result<(), String> {
        match self {
            Self::Active(s) => s.delete().await.map_err(|e| e.to_string()),
            Self::Restored(s) => s.delete().await.map_err(|e| e.to_string()),
        }
    }
    pub fn handoff(self, store: &Store) -> Result<String, String> {
        let session = match self {
            Self::Active(s) => s,
            Self::Restored(s) => s.into_session().map_err(|e| e.to_string())?,
        };
        let id = ContinuationId::new(super::identity("continuation")).unwrap();
        session
            .handoff(id.clone(), store)
            .map_err(|e| e.to_string())?;
        Ok(id.to_string())
    }
}
