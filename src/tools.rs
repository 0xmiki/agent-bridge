//! Typed application operations with host-owned, revision-specific grants.
use crate::{ActorId, SessionId, SlotId};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt, future::Future, pin::Pin, sync::Arc};
pub use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRef {
    pub name: String,
    pub revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolScope {
    pub session: SessionId,
    pub slot: SlotId,
}
#[derive(Debug, Clone)]
pub struct ToolGrant {
    pub issuer: ActorId,
    pub subject: ActorId,
    pub scope: ToolScope,
    pub tools: Vec<ToolRef>,
}
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub reference: ToolRef,
    pub description: String,
    pub input_schema: Value,
}
#[derive(Clone)]
pub struct ToolInvocation {
    pub host: ActorId,
    pub actor: ActorId,
    pub scope: ToolScope,
    pub cancellation: CancellationToken,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    InvalidDefinition,
    DuplicateDefinition,
    NotGranted,
    UnknownTool,
    InvalidArguments(String),
    Handler(String),
    InvalidResult(String),
    Cancelled,
}
impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "application tool: {self:?}")
    }
}
impl Error for ToolError {}

type HandlerFuture = Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>>;
type Handler = dyn Fn(ToolInvocation, Value) -> HandlerFuture + Send + Sync;
struct Entry {
    definition: ToolDefinition,
    handler: Arc<Handler>,
}
#[derive(Default)]
pub struct ToolRegistry {
    entries: BTreeMap<String, Entry>,
}

impl ToolRegistry {
    pub fn register<I, O, F, Fut>(
        &mut self,
        reference: ToolRef,
        description: impl Into<String>,
        handler: F,
    ) -> Result<(), ToolError>
    where
        I: DeserializeOwned + JsonSchema + Send + 'static,
        O: Serialize + Send + 'static,
        F: Fn(ToolInvocation, I) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, ToolError>> + Send + 'static,
    {
        let description = description.into();
        if reference.name.is_empty()
            || reference.name.len() > 128
            || !reference
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
            || reference.revision.trim().is_empty()
            || description.trim().is_empty()
        {
            return Err(ToolError::InvalidDefinition);
        }
        if self.entries.contains_key(&reference.name) {
            return Err(ToolError::DuplicateDefinition);
        }
        let input_schema = serde_json::to_value(schemars::schema_for!(I))
            .map_err(|_| ToolError::InvalidDefinition)?;
        if input_schema["type"] != "object" {
            return Err(ToolError::InvalidDefinition);
        }
        let handler = Arc::new(handler);
        let entry = Entry {
            definition: ToolDefinition {
                reference: reference.clone(),
                description,
                input_schema,
            },
            handler: Arc::new(move |context, arguments| {
                let handler = handler.clone();
                Box::pin(async move {
                    if !arguments.is_object() {
                        return Err(ToolError::InvalidArguments(
                            "arguments must be an object".into(),
                        ));
                    }
                    let input = serde_json::from_value(arguments)
                        .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
                    let output = handler(context, input).await?;
                    serde_json::to_value(output)
                        .map_err(|e| ToolError::InvalidResult(e.to_string()))
                })
            }),
        };
        self.entries.insert(reference.name, entry);
        Ok(())
    }

    fn authorize_scope(grant: &ToolGrant, invocation: &ToolInvocation) -> Result<(), ToolError> {
        if grant.issuer != invocation.host
            || grant.subject != invocation.actor
            || grant.scope != invocation.scope
        {
            return Err(ToolError::NotGranted);
        }
        Ok(())
    }

    /// Only granted, registered revisions are discoverable in this invocation scope.
    pub fn catalog(
        &self,
        grant: &ToolGrant,
        invocation: &ToolInvocation,
    ) -> Result<Vec<ToolDefinition>, ToolError> {
        Self::authorize_scope(grant, invocation)?;
        Ok(self
            .entries
            .values()
            .filter(|entry| grant.tools.contains(&entry.definition.reference))
            .map(|entry| entry.definition.clone())
            .collect())
    }

    pub async fn invoke(
        &self,
        name: &str,
        arguments: Value,
        grant: &ToolGrant,
        invocation: ToolInvocation,
    ) -> Result<Value, ToolError> {
        Self::authorize_scope(grant, &invocation)?;
        if !grant.tools.iter().any(|tool| tool.name == name) {
            return Err(ToolError::NotGranted);
        }
        let entry = self.entries.get(name).ok_or(ToolError::UnknownTool)?;
        if !grant.tools.contains(&entry.definition.reference) {
            return Err(ToolError::NotGranted);
        }
        if invocation.cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let cancellation = invocation.cancellation.clone();
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(ToolError::Cancelled),
            result = (entry.handler)(invocation, arguments) => result,
        }
    }
}
