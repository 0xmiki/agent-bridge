use super::{Output, identity, storage};
use agent_bridge::acp::McpServer;
use agent_bridge::records::{Draft, Payload, RecordState, RecordStore};
use agent_bridge::tools::{
    CancellationToken, ToolDefinition, ToolError, ToolInvocation, ToolRegistry,
};
use agent_bridge::{ActorId, RecordId, ToolGrant, ToolRef, ToolScope};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, oneshot};

#[cfg(unix)]
mod transport;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    pub revision: String,
    pub description: String,
    pub input_schema: Value,
}
pub struct Hub {
    registry: Arc<ToolRegistry>,
    broker: Arc<Broker>,
    definitions: Vec<Definition>,
}
struct Broker {
    output: Output,
    bindings: Mutex<HashMap<(String, String), Arc<Bound>>>,
    pending: Mutex<HashMap<String, Pending>>,
    capacity: Arc<Semaphore>,
    timeout: Duration,
}
struct Bound {
    id: String,
    scope: ToolScope,
    store: Arc<storage::Store>,
    lifetime: CancellationToken,
    activity: Mutex<Option<CancellationToken>>,
    capacity: Arc<Semaphore>,
    retired: AtomicBool,
}
struct Pending {
    binding: String,
    session: String,
    slot: String,
    response: oneshot::Sender<Value>,
    question_owner: QuestionOwner,
}
#[derive(Clone)]
pub struct QuestionOwner {
    pub call: String,
    pub binding: String,
    pub scope: ToolScope,
    pub store: Arc<storage::Store>,
    pub cancellation: CancellationToken,
}
pub struct Binding {
    pub id: String,
    pub mcp: McpServer,
    state: Arc<Bound>,
    broker: Arc<Broker>,
    #[cfg(unix)]
    _transport: transport::Transport,
}
impl Binding {
    pub fn begin(&self) -> Result<(), String> {
        self.end();
        if self.state.retired.load(Ordering::SeqCst) {
            return Err(
                "tool binding retired after an uncertain invocation; create a fresh session".into(),
            );
        }
        *self.state.activity.lock().unwrap() = Some(CancellationToken::new());
        Ok(())
    }
    pub fn end(&self) {
        if let Some(token) = self.state.activity.lock().unwrap().take() {
            if self.state.capacity.available_permits() != 4 {
                self.state.retired.store(true, Ordering::SeqCst);
            }
            token.cancel();
        }
    }
}
impl Drop for Binding {
    fn drop(&mut self) {
        self.end();
        self.state.lifetime.cancel();
        self.broker.bindings.lock().unwrap().remove(&(
            self.state.scope.session.to_string(),
            self.state.scope.slot.to_string(),
        ));
    }
}

// JSON crossing into JS must not silently round an integer or grow without bound.
pub fn valid_json(value: &Value, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    match value {
        Value::Array(items) => items.iter().all(|v| valid_json(v, depth + 1)),
        Value::Object(fields) => fields.values().all(|v| valid_json(v, depth + 1)),
        Value::Number(n) => n.as_f64().is_some_and(|v| {
            v.is_finite() && (v.fract() != 0.0 || v.abs() <= 9_007_199_254_740_991.0)
        }),
        _ => true,
    }
}
impl Hub {
    pub fn new(
        definitions: Vec<Definition>,
        timeout_ms: u64,
        output: Output,
    ) -> Result<Self, String> {
        if definitions.len() > 16 || !(50..=300000).contains(&timeout_ms) {
            return Err("tool limits exceeded".into());
        }
        let broker = Arc::new(Broker {
            output,
            bindings: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            capacity: Arc::new(Semaphore::new(16)),
            timeout: Duration::from_millis(timeout_ms),
        });
        let mut registry = ToolRegistry::default();
        for definition in &definitions {
            let broker = broker.clone();
            let reference = ToolRef {
                name: definition.name.clone(),
                revision: definition.revision.clone(),
            };
            let selected = reference.clone();
            registry
                .register_dynamic(
                    ToolDefinition {
                        reference,
                        description: definition.description.clone(),
                        input_schema: definition.input_schema.clone(),
                    },
                    move |context, input| {
                        let broker = broker.clone();
                        let reference = selected.clone();
                        async move { broker.invoke(reference, context, input).await }
                    },
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(Self {
            registry: Arc::new(registry),
            broker,
            definitions,
        })
    }
    pub fn validate(&self, tools: &[ToolRef]) -> Result<(), String> {
        let mut names = std::collections::HashSet::new();
        if tools.len() > 16
            || tools.iter().any(|tool| {
                !names.insert(&tool.name)
                    || !self
                        .definitions
                        .iter()
                        .any(|d| d.name == tool.name && d.revision == tool.revision)
            })
        {
            return Err("unknown, duplicate, or stale tool grant".into());
        }
        Ok(())
    }
    pub async fn bind(
        &self,
        scope: ToolScope,
        tools: Vec<ToolRef>,
        store: Arc<storage::Store>,
    ) -> Result<Option<Binding>, String> {
        self.validate(&tools)?;
        if tools.is_empty() {
            return Ok(None);
        }
        #[cfg(not(unix))]
        {
            let _ = (scope, store);
            return Err("hosted tools currently require Unix local sockets".into());
        }
        #[cfg(unix)]
        {
            let state = Arc::new(Bound {
                id: identity("binding"),
                scope: scope.clone(),
                store,
                lifetime: CancellationToken::new(),
                activity: Mutex::new(None),
                capacity: Arc::new(Semaphore::new(4)),
                retired: AtomicBool::new(false),
            });
            let grant = ToolGrant {
                issuer: ActorId::new("host").unwrap(),
                subject: ActorId::new("assistant").unwrap(),
                scope: scope.clone(),
                tools,
            };
            let (transport, mcp) =
                transport::start(self.registry.clone(), grant, state.clone()).await?;
            self.broker.bindings.lock().unwrap().insert(
                (scope.session.to_string(), scope.slot.to_string()),
                state.clone(),
            );
            Ok(Some(Binding {
                id: state.id.clone(),
                mcp,
                state,
                broker: self.broker.clone(),
                _transport: transport,
            }))
        }
    }
    pub fn reply(&self, params: &Value) -> Result<(), String> {
        let call = super::string(params, "call_id")?;
        let mut pending = self.broker.pending.lock().unwrap();
        let entry = pending.get(call).ok_or("tool call is not pending")?;
        if params["binding_id"].as_str() != Some(&entry.binding)
            || params["session_id"].as_str() != Some(&entry.session)
            || params["slot_id"].as_str() != Some(&entry.slot)
        {
            return Err("tool result belongs to another binding or scope".into());
        }
        let result = &params["outcome"];
        if result.to_string().len() > 65536
            || !valid_json(result, 0)
            || match result["kind"].as_str() {
                Some("success") => result.get("value").is_none(),
                Some("error") => result["message"].as_str().is_none(),
                _ => true,
            }
        {
            return Err("invalid tool result".into());
        }
        let entry = pending.remove(call).unwrap();
        entry
            .response
            .send(result.clone())
            .map_err(|_| "tool call already ended".into())
    }
    pub fn question_owner(&self, params: &Value) -> Result<QuestionOwner, String> {
        let pending = self.broker.pending.lock().unwrap();
        let entry = pending
            .get(super::string(params, "call_id")?)
            .ok_or("invocation is not pending")?;
        if params["binding_id"].as_str() != Some(&entry.binding)
            || params["session_id"].as_str() != Some(&entry.session)
            || params["slot_id"].as_str() != Some(&entry.slot)
            || entry.question_owner.cancellation.is_cancelled()
        {
            return Err("question belongs to another or ended invocation".into());
        }
        Ok(entry.question_owner.clone())
    }
}

struct CallGuard {
    broker: Arc<Broker>,
    bound: Arc<Bound>,
    call: String,
    reference: ToolRef,
    settled: bool,
    activity: CancellationToken,
    question_cancellation: CancellationToken,
}
impl CallGuard {
    fn receipt(&self, state: &str, fields: Value) -> Result<(), ToolError> {
        let mut data = json!({"version":1,"invocation_id":self.call,"binding_id":self.bound.id,"scope":self.bound.scope,"tool":self.reference,
            "issuer":"host","subject":"assistant","state":state});
        data.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        self.bound
            .store
            .insert(Draft {
                id: RecordId::new(format!("{}-{state}", self.call)).unwrap(),
                session_id: self.bound.scope.session.clone(),
                run_id: None,
                actor: ActorId::new("host").unwrap(),
                reply_to_id: None,
                source: None,
                state: RecordState::Complete,
                payload: Payload::Extension {
                    namespace: "agent_bridge".into(),
                    name: "tool_invocation".into(),
                    data,
                },
            })
            .map_err(|e| ToolError::Handler(format!("tool evidence unavailable: {e}")))?;
        Ok(())
    }
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        self.question_cancellation.cancel();
        self.broker.pending.lock().unwrap().remove(&self.call);
        if !self.settled {
            // MCP does not attest to a call's parent run. Retire this binding so
            // delayed old work cannot acquire authority from a later run.
            self.bound.retired.store(true, Ordering::SeqCst);
            self.activity.cancel();
            if let Some(current) = self.bound.activity.lock().unwrap().take() {
                current.cancel();
            }
            self.broker.output.emit(json!({"event":"tool_cancel","call_id":self.call,"binding_id":self.bound.id,"session_id":self.bound.scope.session.as_str(),"slot_id":self.bound.scope.slot.as_str()}));
            let _ = self.receipt(
                "unknown",
                json!({"reason":"invocation ended without an acknowledged, recorded result"}),
            );
        }
    }
}
impl Broker {
    async fn invoke(
        self: Arc<Self>,
        reference: ToolRef,
        context: ToolInvocation,
        input: Value,
    ) -> Result<Value, ToolError> {
        if input.to_string().len() > 65536 || !valid_json(&input, 0) {
            return Err(ToolError::InvalidArguments(
                "input exceeds JSON transport limits".into(),
            ));
        }
        let bound = self
            .bindings
            .lock()
            .unwrap()
            .get(&(
                context.scope.session.to_string(),
                context.scope.slot.to_string(),
            ))
            .cloned()
            .ok_or(ToolError::NotGranted)?;
        let (activity, _global, _local) = {
            // Admission and activity closure share this lock, so end() cannot
            // miss a call that has already reserved execution capacity.
            let current = bound.activity.lock().unwrap();
            let activity = current.clone().ok_or(ToolError::NotGranted)?;
            if activity.is_cancelled() || bound.retired.load(Ordering::SeqCst) {
                return Err(ToolError::NotGranted);
            }
            let global = self
                .capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| ToolError::Handler("tool call capacity reached".into()))?;
            let local = bound
                .capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| ToolError::Handler("binding call capacity reached".into()))?;
            (activity, global, local)
        };
        let mut guard = CallGuard {
            broker: self.clone(),
            bound: bound.clone(),
            call: identity("invocation"),
            reference: reference.clone(),
            settled: false,
            activity: activity.clone(),
            question_cancellation: CancellationToken::new(),
        };
        guard.receipt("dispatch_attempted", json!({"input":input}))?;
        if activity.is_cancelled()
            || bound.lifetime.is_cancelled()
            || self.output.stop.is_cancelled()
            || context.cancellation.is_cancelled()
        {
            return Err(ToolError::Cancelled);
        }
        let (response, receive) = oneshot::channel();
        self.pending.lock().unwrap().insert(
            guard.call.clone(),
            Pending {
                binding: bound.id.clone(),
                session: bound.scope.session.to_string(),
                slot: bound.scope.slot.to_string(),
                response,
                question_owner: QuestionOwner {
                    call: guard.call.clone(),
                    binding: bound.id.clone(),
                    scope: bound.scope.clone(),
                    store: bound.store.clone(),
                    cancellation: guard.question_cancellation.clone(),
                },
            },
        );
        self.output.emit(json!({"event":"tool_call","call_id":guard.call,"binding_id":bound.id,"session_id":bound.scope.session.as_str(),"slot_id":bound.scope.slot.as_str(),"tool":reference,"input":input}));
        let outcome = tokio::select! {
            biased;
            _ = self.output.stop.cancelled() => return Err(ToolError::Cancelled),
            _ = bound.lifetime.cancelled() => return Err(ToolError::Cancelled),
            _ = activity.cancelled() => return Err(ToolError::Cancelled),
            _ = context.cancellation.cancelled() => return Err(ToolError::Cancelled),
            result = tokio::time::timeout(self.timeout,receive) => result.map_err(|_| ToolError::Handler("application tool timed out; outcome unknown".into()))?.map_err(|_| ToolError::Cancelled)?,
        };
        guard.receipt("returned", json!({"outcome":outcome}))?;
        guard.settled = true;
        if outcome["kind"] == "success" {
            Ok(outcome["value"].clone())
        } else {
            Err(ToolError::Handler(
                outcome["message"].as_str().unwrap().into(),
            ))
        }
    }
}

pub fn helper() -> Result<(), String> {
    #[cfg(unix)]
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?
            .block_on(transport::helper())
    }
    #[cfg(not(unix))]
    {
        Err("hosted tools currently require Unix local sockets".into())
    }
}
