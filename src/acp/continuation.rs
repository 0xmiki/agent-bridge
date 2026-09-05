use super::{AcpConnection, AcpError, AcpSession, McpServer};
use crate::ContinuationId;
use crate::records::{Continuation, ContinuationRecord, ContinuationState, ContinuationStore};
use agent_client_protocol::schema::v1::{
    NewSessionResponse, ResumeSessionRequest, SessionId as NativeSessionId,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

const ADAPTER: &str = "acp-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcpData {
    version: u32,
    cwd: PathBuf,
    agent_name: String,
    agent_version: String,
}

impl AcpConnection {
    fn scope(&self) -> Result<&str, AcpError> {
        self.continuation_scope
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or(AcpError::ContinuationScopeRequired)
    }

    /// Claim and resume a saved native session. Never replays prompts or falls back
    /// to session/new or session/load. Preflight failures do not claim the handle;
    /// failures after claiming leave it claimed because setup may have occurred.
    pub async fn resume_saved<S: ContinuationStore>(
        &self,
        store: &S,
        id: &ContinuationId,
        mcp_servers: Vec<McpServer>,
    ) -> Result<AcpSession<'_>, AcpError> {
        if self.is_closed() {
            return Err(AcpError::Closed);
        }
        if self
            .info
            .agent_capabilities
            .session_capabilities
            .resume
            .is_none()
        {
            return Err(AcpError::ResumeUnsupported);
        }
        let saved = store.get_continuation(id).map_err(AcpError::Store)?;
        let descriptor = &saved.continuation;
        let data: AcpData = serde_json::from_value(descriptor.data.clone())
            .map_err(|_| AcpError::IncompatibleContinuation)?;
        let agent = self
            .info
            .agent_info
            .as_ref()
            .ok_or(AcpError::IncompatibleContinuation)?;
        if descriptor.adapter != ADAPTER
            || descriptor.scope != self.scope()?
            || data.version != 1
            || agent.name != data.agent_name
            || agent.version != data.agent_version
        {
            return Err(AcpError::IncompatibleContinuation);
        }
        if !data.cwd.is_absolute() {
            return Err(AcpError::InvalidWorkingDirectory);
        }
        self.validate_mcp(&mcp_servers)?;
        let native = NativeSessionId::new(descriptor.native_key.clone());
        // Reserve before dispatch; uncertain setup leaves a tombstone on this
        // connection so a late reply cannot collide with another session handle.
        if !self.routes.lock().unwrap().sessions.insert(native.clone()) {
            return Err(AcpError::SessionUnavailable);
        }
        if let Err(error) = store.claim_continuation(id) {
            self.routes.lock().unwrap().sessions.remove(&native);
            return Err(AcpError::Store(error));
        }
        let (info, configuration) = self
            .setup_configuration(
                ResumeSessionRequest::new(native.clone(), data.cwd.clone())
                    .mcp_servers(mcp_servers),
                move |response| {
                    NewSessionResponse::new(native)
                        .modes(response.modes)
                        .config_options(response.config_options)
                        .meta(response.meta)
                },
            )
            .await?;
        Ok(AcpSession {
            connection: self,
            session_id: descriptor.session_id.clone(),
            slot_id: descriptor.slot_id.clone(),
            info,
            retired: false,
            cwd: data.cwd,
            predecessor: Some(id.clone()),
            quiescent: true,
            configuration,
        })
    }
}

impl AcpSession<'_> {
    /// Relinquish this idle handle and persist a single-use native continuation.
    /// The source handle is consumed even on error. No credentials or MCP environment
    /// are saved. Native data is not a snapshot of application history.
    pub fn handoff<S: ContinuationStore>(
        self,
        id: ContinuationId,
        store: &S,
    ) -> Result<Arc<ContinuationRecord>, AcpError> {
        if self.retired || !self.quiescent {
            return Err(AcpError::UnsafeHandoff);
        }
        self.configuration.lock().unwrap().for_run()?;
        if self.connection.is_closed() {
            return Err(AcpError::Closed);
        }
        if self
            .connection
            .info
            .agent_capabilities
            .session_capabilities
            .resume
            .is_none()
        {
            return Err(AcpError::ResumeUnsupported);
        }
        let scope = self.connection.scope()?;
        let agent = self
            .connection
            .info
            .agent_info
            .as_ref()
            .ok_or(AcpError::IncompatibleContinuation)?;
        let data = AcpData {
            version: 1,
            cwd: self.cwd,
            agent_name: agent.name.clone(),
            agent_version: agent.version.clone(),
        };
        store
            .create_session(self.session_id.clone())
            .map_err(AcpError::Store)?;
        let saved = store
            .save_continuation(Continuation {
                id,
                session_id: self.session_id,
                slot_id: self.slot_id,
                adapter: ADAPTER.into(),
                scope: scope.into(),
                native_key: self.info.session_id.to_string(),
                predecessor: self.predecessor,
                data: serde_json::to_value(data).map_err(|_| AcpError::IncompatibleContinuation)?,
            })
            .map_err(AcpError::Store)?;
        if saved.state != ContinuationState::Available || !saved.latest {
            return Err(AcpError::Store(
                crate::records::StoreError::ContinuationClaimed,
            ));
        }
        self.connection
            .routes
            .lock()
            .unwrap()
            .sessions
            .remove(&self.info.session_id);
        Ok(saved)
    }
}
