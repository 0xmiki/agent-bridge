use super::{AcpError, AcpEvent, AcpRun, PermissionId};
use crate::records::{
    CompletionReason, Draft, MessageKind, Payload, PermissionOption, RecordState, RecordStore,
    Snapshot, SourceRef, StoreError, ToolActivity, ToolStatus,
};
use crate::{ActorId, ConfigValues, Content, Message, RecordId, Run, RunSpec};
use agent_client_protocol::schema::v1::{
    ContentBlock, SessionUpdate, StopReason, ToolCall, ToolCallStatus,
};
use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fmt,
    sync::Arc,
};

/// Attribution is supplied by the application, independently of the provider.
#[derive(Debug, Clone)]
pub struct RecordActors {
    pub user: ActorId,
    pub agent: ActorId,
    /// The host that submits decisions, including automatic cancellation.
    pub host: ActorId,
}

#[derive(Debug)]
pub enum RecordingError {
    NativeStructuredOutputUnsupported,
    Context(crate::context::ContextError),
    UnsupportedContext(&'static str),
    Store(StoreError),
    Agent(AcpError),
    RunAlreadyRecorded,
    Closed,
    MissingPermission,
    Serialization(serde_json::Error),
}
impl fmt::Display for RecordingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NativeStructuredOutputUnsupported => {
                f.write_str("this ACP driver cannot guarantee native structured-output enforcement")
            }
            Self::Context(e) => e.fmt(f),
            Self::UnsupportedContext(message) => f.write_str(message),
            Self::Store(e) => e.fmt(f),
            Self::Agent(e) => e.fmt(f),
            Self::RunAlreadyRecorded => {
                f.write_str("run identity is already registered; choose a new ID")
            }
            Self::Closed => f.write_str("recording stopped after an error"),
            Self::MissingPermission => f.write_str("decision has no observed permission request"),
            Self::Serialization(e) => e.fmt(f),
        }
    }
}
impl Error for RecordingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Context(e) => Some(e),
            Self::Store(e) => Some(e),
            Self::Agent(e) => Some(e),
            Self::Serialization(e) => Some(e),
            _ => None,
        }
    }
}
impl From<StoreError> for RecordingError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}
impl From<crate::context::ContextError> for RecordingError {
    fn from(error: crate::context::ContextError) -> Self {
        Self::Context(error)
    }
}
impl From<AcpError> for RecordingError {
    fn from(e: AcpError) -> Self {
        Self::Agent(e)
    }
}
impl From<serde_json::Error> for RecordingError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e)
    }
}

struct Working {
    stored: Arc<Snapshot>,
    payload: Payload,
    dirty: bool,
}

pub(super) struct Recorder<'store, S: RecordStore> {
    store: &'store S,
    spec: RunSpec,
    actors: RecordActors,
    records: Vec<Working>,
    messages: HashMap<(MessageKind, String), usize>,
    anonymous: Option<(MessageKind, usize)>,
    tools: HashMap<String, (usize, ToolCall)>,
    permissions: HashMap<PermissionId, usize>,
    finished: bool,
    last_configuration: Option<ConfigValues>,
    input_receipt: bool,
    input_version: u64,
    input_response: bool,
}

impl<'store, S: RecordStore> Recorder<'store, S> {
    pub(super) fn new(
        store: &'store S,
        spec: RunSpec,
        input: &str,
        actors: RecordActors,
    ) -> Result<Self, RecordingError> {
        store.create_session(spec.session_id.clone())?;
        if !store.register_run(spec.clone())? {
            return Err(RecordingError::RunAlreadyRecorded);
        }
        let last_configuration = spec.config.confirmed.clone();
        let mut this = Self {
            store,
            spec,
            actors,
            records: Vec::new(),
            messages: HashMap::new(),
            anonymous: None,
            tools: HashMap::new(),
            permissions: HashMap::new(),
            finished: false,
            last_configuration,
            input_receipt: false,
            input_version: 1,
            input_response: false,
        };
        this.insert(
            Payload::Message {
                kind: MessageKind::User,
                message: Message {
                    content: vec![Content::Text(input.to_owned())],
                },
            },
            this.actors.user.clone(),
            None,
            None,
            RecordState::Complete,
        )?;
        Ok(this)
    }

    pub(super) fn prepare_input(
        &mut self,
        receipt: serde_json::Value,
    ) -> Result<(), RecordingError> {
        self.input_version = receipt["version"]
            .as_u64()
            .expect("internal receipt version");
        self.input_evidence(receipt)?;
        self.input_receipt = true;
        Ok(())
    }

    pub(super) fn restoration(&mut self, data: serde_json::Value) -> Result<(), RecordingError> {
        self.insert(
            Payload::Extension {
                namespace: "agent_bridge".into(),
                name: "restoration".into(),
                data,
            },
            self.actors.host.clone(),
            None,
            None,
            RecordState::Complete,
        )?;
        Ok(())
    }

    #[cfg(feature = "structured")]
    pub(super) fn result_evidence(
        &mut self,
        name: &str,
        data: serde_json::Value,
    ) -> Result<(), RecordingError> {
        self.insert(
            Payload::Extension {
                namespace: "agent_bridge".into(),
                name: name.into(),
                data,
            },
            self.actors.host.clone(),
            None,
            None,
            RecordState::Complete,
        )?;
        Ok(())
    }

    fn input_evidence(&mut self, data: serde_json::Value) -> Result<(), RecordingError> {
        self.insert(
            Payload::Extension {
                namespace: "agent_bridge".into(),
                name: "input_receipt".into(),
                data,
            },
            self.actors.host.clone(),
            None,
            None,
            RecordState::Complete,
        )?;
        Ok(())
    }

    pub(super) fn input_dispatch_attempted(&mut self) -> Result<(), RecordingError> {
        self.input_evidence(
            serde_json::json!({"version":self.input_version,"state":"dispatch_attempted"}),
        )
    }

    fn draft(
        &self,
        payload: Payload,
        actor: ActorId,
        reply_to_id: Option<RecordId>,
        source: Option<SourceRef>,
        state: RecordState,
    ) -> Draft {
        // Length-prefix the caller ID so separators within it cannot cause collisions.
        let run = self.spec.id.as_str();
        Draft {
            id: RecordId::new(format!(
                "run/{}:{run}/record/{}",
                run.len(),
                self.records.len()
            ))
            .unwrap(),
            session_id: self.spec.session_id.clone(),
            run_id: Some(self.spec.id.clone()),
            actor,
            reply_to_id,
            source,
            payload,
            state,
        }
    }

    fn insert(
        &mut self,
        payload: Payload,
        actor: ActorId,
        reply: Option<RecordId>,
        source: Option<SourceRef>,
        state: RecordState,
    ) -> Result<usize, RecordingError> {
        let draft = self.draft(payload.clone(), actor, reply, source, state);
        let stored = self.store.insert(draft)?;
        let index = self.records.len();
        self.records.push(Working {
            stored,
            payload,
            dirty: false,
        });
        Ok(index)
    }

    fn extension(&mut self, name: &str, data: serde_json::Value) -> Result<(), RecordingError> {
        self.insert(
            Payload::Extension {
                namespace: "acp".into(),
                name: name.into(),
                data,
            },
            self.actors.agent.clone(),
            None,
            None,
            RecordState::Complete,
        )?;
        Ok(())
    }

    fn message(
        &mut self,
        kind: MessageKind,
        chunk: &agent_client_protocol::schema::v1::ContentChunk,
    ) -> Result<(), RecordingError> {
        let ContentBlock::Text(text) = &chunk.content else {
            self.anonymous = None;
            return self.extension("message_content", serde_json::to_value(chunk)?);
        };
        let native = chunk.message_id.as_ref().map(ToString::to_string);
        let existing = match &native {
            Some(id) => self.messages.get(&(kind, id.clone())).copied(),
            None => self
                .anonymous
                .filter(|(last, _)| last == &kind)
                .map(|(_, index)| index),
        };
        let index = if let Some(index) = existing {
            index
        } else {
            let index = self.insert(
                Payload::Message {
                    kind,
                    message: Message {
                        content: vec![Content::Text(String::new())],
                    },
                },
                self.actors.agent.clone(),
                None,
                native.as_ref().map(|id| SourceRef {
                    namespace: "acp".into(),
                    id: id.clone(),
                }),
                RecordState::Open,
            )?;
            if let Some(id) = &native {
                self.messages.insert((kind, id.clone()), index);
            }
            index
        };
        self.anonymous = if native.is_none() {
            Some((kind, index))
        } else {
            None
        };
        let working = &mut self.records[index];
        if let Payload::Message { message, .. } = &mut working.payload
            && let Some(Content::Text(buffer)) = message.content.last_mut()
        {
            buffer.push_str(&text.text);
        }
        working.dirty = true;
        Ok(())
    }

    fn tool(&mut self, call: ToolCall) -> Result<(), RecordingError> {
        let key = call.tool_call_id.to_string();
        let payload = tool_payload(&call)?;
        let index = if let Some((index, _)) = self.tools.get(&key) {
            let index = *index;
            self.records[index].payload = payload;
            self.records[index].dirty = true;
            index
        } else {
            self.insert(
                payload,
                self.actors.agent.clone(),
                None,
                Some(SourceRef {
                    namespace: "acp".into(),
                    id: key.clone(),
                }),
                RecordState::Open,
            )?
        };
        self.tools.insert(key, (index, call));
        Ok(())
    }

    fn tool_update(
        &mut self,
        update: &agent_client_protocol::schema::v1::ToolCallUpdate,
    ) -> Result<(), RecordingError> {
        let key = update.tool_call_id.to_string();
        let mut call = self
            .tools
            .get(&key)
            .map(|(_, call)| call.clone())
            .unwrap_or_else(|| ToolCall::new(key.clone(), "Tool activity"));
        call.update(update.fields.clone());
        if update.meta.is_some() {
            call.meta = update.meta.clone();
        }
        self.tool(call)
    }

    pub(super) fn observe(&mut self, event: &AcpEvent) -> Result<(), RecordingError> {
        if self.finished {
            return Err(RecordingError::Closed);
        }
        if !matches!(
            event,
            AcpEvent::Update(
                SessionUpdate::AgentMessageChunk(_) | SessionUpdate::AgentThoughtChunk(_)
            )
        ) {
            self.anonymous = None;
        }
        match event {
            AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                self.message(MessageKind::Agent, chunk)
            }
            AcpEvent::Update(SessionUpdate::AgentThoughtChunk(chunk)) => {
                self.message(MessageKind::Reasoning, chunk)
            }
            AcpEvent::Update(SessionUpdate::ToolCall(call)) => self.tool(call.clone()),
            AcpEvent::Update(SessionUpdate::ToolCallUpdate(update)) => self.tool_update(update),
            AcpEvent::Update(SessionUpdate::ConfigOptionUpdate(update)) => {
                let report = super::configuration::normalize(&update.config_options)
                    .ok()
                    .map(|options| {
                        options
                            .into_iter()
                            .map(|option| (option.id, option.current))
                            .collect::<ConfigValues>()
                    });
                if report == self.last_configuration {
                    return Ok(());
                }
                self.last_configuration = report.clone();
                self.insert(
                    Payload::Extension {
                        namespace: "agent_bridge".into(),
                        name: "configuration_report".into(),
                        data: serde_json::json!({"confirmed": report}),
                    },
                    self.actors.agent.clone(),
                    None,
                    None,
                    RecordState::Complete,
                )?;
                Ok(())
            }
            AcpEvent::Update(other) => {
                self.extension("session_update", serde_json::to_value(other)?)
            }
            AcpEvent::Permission { id, request } => {
                self.tool_update(&request.tool_call)?;
                let tool = self.tools[&request.tool_call.tool_call_id.to_string()].0;
                let options = request
                    .options
                    .iter()
                    .map(|option| {
                        Ok(PermissionOption {
                            id: option.option_id.to_string(),
                            label: option.name.clone(),
                            effect: serde_json::to_value(option.kind)?
                                .as_str()
                                .unwrap_or("unknown")
                                .to_owned(),
                        })
                    })
                    .collect::<Result<Vec<_>, serde_json::Error>>()?;
                let index = self.insert(
                    Payload::Permission {
                        title: request
                            .tool_call
                            .fields
                            .title
                            .clone()
                            .unwrap_or_else(|| "Tool permission".into()),
                        options,
                    },
                    self.actors.agent.clone(),
                    Some(self.records[tool].stored.record.id.clone()),
                    None,
                    RecordState::Open,
                )?;
                self.permissions.insert(id.clone(), index);
                Ok(())
            }
            AcpEvent::PermissionResolved {
                id,
                outcome,
                delivery,
            } => {
                let index = *self
                    .permissions
                    .get(id)
                    .ok_or(RecordingError::MissingPermission)?;
                let request = &self.records[index].stored;
                let request_id = request.record.id.clone();
                let payload = Payload::Decision {
                    outcome: outcome.clone(),
                    delivery: *delivery,
                };
                let draft = self.draft(
                    payload.clone(),
                    self.actors.host.clone(),
                    Some(request_id.clone()),
                    None,
                    RecordState::Complete,
                );
                let response = self.store.resolve(&request_id, request.revision, draft)?;
                self.records[index].stored = self.store.get(&request_id)?;
                self.records.push(Working {
                    stored: response,
                    payload,
                    dirty: false,
                });
                Ok(())
            }
            AcpEvent::Finished(reason) => {
                if self.input_receipt {
                    self.input_evidence(serde_json::json!({"version":self.input_version,"state":"response_received","stop_reason":reason}))?;
                    self.input_response = true;
                }
                let normalized = match reason {
                    StopReason::EndTurn => CompletionReason::Completed,
                    StopReason::Refusal => CompletionReason::Refused,
                    StopReason::MaxTokens => CompletionReason::TokenLimit,
                    StopReason::MaxTurnRequests => CompletionReason::StepLimit,
                    StopReason::Cancelled => CompletionReason::Cancelled,
                    other => CompletionReason::Other(serde_json::to_string(other)?),
                };
                self.insert(
                    Payload::RunFinished { reason: normalized },
                    self.actors.agent.clone(),
                    None,
                    None,
                    RecordState::Complete,
                )?;
                self.finish(matches!(reason, StopReason::EndTurn | StopReason::Refusal))
            }
        }
    }

    /// Persist a coarse checkpoint without sealing live records.
    pub(super) fn checkpoint(&mut self) -> Result<(), RecordingError> {
        for working in &mut self.records {
            if working.dirty && !working.stored.state.is_final() {
                working.stored = self.store.checkpoint(
                    &working.stored.record.id,
                    working.stored.revision,
                    working.payload.clone(),
                    RecordState::Open,
                )?;
                working.dirty = false;
            }
        }
        Ok(())
    }

    fn finish(&mut self, complete: bool) -> Result<(), RecordingError> {
        for working in &mut self.records {
            if working.stored.state.is_final() {
                continue;
            }
            let record_complete = match &working.payload {
                Payload::Permission { .. } => false,
                Payload::Tool(tool) => {
                    matches!(tool.status, ToolStatus::Completed | ToolStatus::Failed)
                }
                _ => complete,
            };
            working.stored = self.store.checkpoint(
                &working.stored.record.id,
                working.stored.revision,
                working.payload.clone(),
                if record_complete {
                    RecordState::Complete
                } else {
                    RecordState::Interrupted
                },
            )?;
            working.dirty = false;
        }
        self.finished = true;
        Ok(())
    }

    pub(super) fn interrupt(&mut self, message: String) -> Result<(), RecordingError> {
        if self.finished {
            return Ok(());
        }
        if self.input_receipt && !self.input_response {
            self.input_evidence(
                serde_json::json!({"version":self.input_version,"state":"unknown"}),
            )?;
        }
        self.insert(
            Payload::Failure { message },
            self.actors.host.clone(),
            None,
            None,
            RecordState::Complete,
        )?;
        self.finish(false)
    }

    fn snapshot(&self) -> Vec<Snapshot> {
        self.records
            .iter()
            .map(|working| {
                let mut snapshot = (*working.stored).clone();
                snapshot.record.payload = working.payload.clone();
                snapshot
            })
            .collect()
    }
}

fn tool_payload(call: &ToolCall) -> Result<Payload, RecordingError> {
    let status = match call.status {
        ToolCallStatus::Pending => ToolStatus::Pending,
        ToolCallStatus::InProgress => ToolStatus::Running,
        ToolCallStatus::Completed => ToolStatus::Completed,
        ToolCallStatus::Failed => ToolStatus::Failed,
        _ => ToolStatus::Unknown,
    };
    let mut details = serde_json::to_value(call)?;
    if let Some(object) = details.as_object_mut() {
        for key in ["toolCallId", "title", "status", "rawInput", "rawOutput"] {
            object.remove(key);
        }
    }
    Ok(Payload::Tool(ToolActivity {
        title: call.title.clone(),
        status,
        input: call.raw_input.clone(),
        output: call.raw_output.clone(),
        extensions: BTreeMap::from([("acp".into(), details)]),
    }))
}

/// Streams ACP events while recording portable snapshots through a local store.
/// Drain to completion to observe storage errors. Drop checkpoints partial output
/// on a best-effort basis; it cannot report storage failures to its caller.
pub struct RecordedRun<'session, 'connection, 'store, S: RecordStore> {
    inner: AcpRun<'session, 'connection>,
    recorder: Recorder<'store, S>,
    failed: bool,
}

impl<'session, 'connection, 'store, S: RecordStore> RecordedRun<'session, 'connection, 'store, S> {
    pub(super) fn new(inner: AcpRun<'session, 'connection>, recorder: Recorder<'store, S>) -> Self {
        Self {
            inner,
            recorder,
            failed: false,
        }
    }
    pub fn run(&self) -> &Run {
        self.inner.run()
    }

    #[cfg(feature = "structured")]
    pub(super) fn json_candidate(
        &self,
    ) -> (
        serde_json::Value,
        Result<&str, crate::structured::JsonRejection>,
    ) {
        use crate::structured::JsonRejection;
        let messages: Vec<_> = self
            .recorder
            .records
            .iter()
            .filter(|record| {
                matches!(
                    record.payload,
                    Payload::Message {
                        kind: MessageKind::Agent,
                        ..
                    }
                )
            })
            .collect();
        let sources = serde_json::json!(messages.iter().map(|record| serde_json::json!({"id":record.stored.record.id.as_str(),"revision":record.stored.revision})).collect::<Vec<_>>());
        let candidate = if self.recorder.records.iter().any(|record| matches!(&record.payload, Payload::Extension { namespace, name, .. } if namespace == "acp" && name == "message_content")) {
            Err(JsonRejection::NonTextOutput)
        } else {
            match messages.as_slice() {
                [] => Err(JsonRejection::MissingOutput),
                [record] if record.stored.state == RecordState::Complete => match &record.payload {
                    Payload::Message { message, .. } => match message.content.as_slice() {
                        [Content::Text(text)] => Ok(text.as_str()), _ => Err(JsonRejection::NonTextOutput),
                    }, _ => unreachable!(),
                },
                [_] => Err(JsonRejection::Incomplete("output record is not complete".into())),
                _ => Err(JsonRejection::AmbiguousOutput),
            }
        };
        (sources, candidate)
    }

    #[cfg(feature = "structured")]
    pub(super) fn result_evidence(
        &mut self,
        data: serde_json::Value,
    ) -> Result<(), RecordingError> {
        self.recorder.result_evidence("result_validation", data)
    }
    /// Live payloads with persisted identity/revision. Reading explicitly clones content.
    pub fn snapshot(&self) -> Vec<Snapshot> {
        self.recorder.snapshot()
    }
    pub fn checkpoint(&mut self) -> Result<(), RecordingError> {
        if self.failed {
            return Err(RecordingError::Closed);
        }
        if let Err(error) = self.recorder.checkpoint() {
            self.failed = true;
            let _ = self.inner.cancel();
            return Err(error);
        }
        Ok(())
    }
    pub fn permission_pending(&self, id: &PermissionId) -> bool {
        self.inner.permission_pending(id)
    }
    pub fn respond(
        &mut self,
        id: PermissionId,
        option: Option<&str>,
    ) -> Result<(), RecordingError> {
        if self.failed {
            return Err(RecordingError::Closed);
        }
        self.inner.respond(id, option).map_err(Into::into)
    }
    pub fn cancel(&mut self) -> Result<(), RecordingError> {
        self.inner.cancel().map_err(Into::into)
    }

    pub async fn next(&mut self) -> Result<Option<AcpEvent>, RecordingError> {
        if self.failed {
            return Err(RecordingError::Closed);
        }
        let event = match self.inner.next().await {
            Ok(event) => event,
            Err(error) => {
                self.failed = true;
                self.recorder.interrupt(error.to_string())?;
                return Err(RecordingError::Agent(error));
            }
        };
        if let Some(event) = &event
            && let Err(error) = self.recorder.observe(event)
        {
            self.failed = true;
            let _ = self.inner.cancel();
            return Err(error);
        }
        Ok(event)
    }
}

impl<S: RecordStore> Drop for RecordedRun<'_, '_, '_, S> {
    fn drop(&mut self) {
        if !self.recorder.finished {
            let _ = self.inner.cancel();
            let _ = self
                .recorder
                .interrupt("Recording ended before a complete transcript was saved.".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::MemoryStore;
    use crate::{ContextManifest, RunId, SessionId, SlotId};
    use serde_json::json;

    fn recorder(store: &MemoryStore) -> Recorder<'_, MemoryStore> {
        Recorder::new(
            store,
            RunSpec {
                id: RunId::new("r").unwrap(),
                session_id: SessionId::new("s").unwrap(),
                slot_id: SlotId::new("slot").unwrap(),
                context: ContextManifest::default(),
                config: Default::default(),
                continuation: None,
            },
            "Question",
            RecordActors {
                user: ActorId::new("u").unwrap(),
                agent: ActorId::new("a").unwrap(),
                host: ActorId::new("h").unwrap(),
            },
        )
        .unwrap()
    }
    fn event(value: serde_json::Value) -> AcpEvent {
        AcpEvent::Update(serde_json::from_value(value).unwrap())
    }
    fn chunk(kind: &str, id: &str, text: &str) -> AcpEvent {
        event(json!({"sessionUpdate":kind,"messageId":id,"content":{"type":"text","text":text}}))
    }

    #[test]
    fn message_ids_and_reasoning_channels_do_not_merge_unrelated_output() {
        let store = MemoryStore::default();
        let mut recorder = recorder(&store);
        for event in [
            chunk("agent_message_chunk", "one", "A"),
            chunk("agent_thought_chunk", "one", "R"),
            chunk("agent_message_chunk", "two", "B"),
            chunk("agent_message_chunk", "one", "C"),
        ] {
            recorder.observe(&event).unwrap();
        }
        recorder
            .observe(&AcpEvent::Finished(StopReason::EndTurn))
            .unwrap();
        let messages: Vec<_> = store
            .list(&SessionId::new("s").unwrap(), None, 100)
            .unwrap()
            .into_iter()
            .filter_map(|record| match &record.record.payload {
                Payload::Message { kind, message } => Some((*kind, message.content.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            messages,
            vec![
                (MessageKind::User, vec![Content::Text("Question".into())]),
                (MessageKind::Agent, vec![Content::Text("AC".into())]),
                (MessageKind::Reasoning, vec![Content::Text("R".into())]),
                (MessageKind::Agent, vec![Content::Text("B".into())]),
            ]
        );
    }

    #[test]
    fn partial_tool_updates_preserve_inputs_and_replace_content_collections() {
        let store = MemoryStore::default();
        let mut recorder = recorder(&store);
        recorder.observe(&event(json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Read","rawInput":{"path":"a"},
            "content":[{"type":"content","content":{"type":"text","text":"old"}}]}))).unwrap();
        recorder
            .observe(&event(
                json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed",
            "rawOutput":{"ok":true},"content":[]}),
            ))
            .unwrap();
        recorder
            .observe(&AcpEvent::Finished(StopReason::EndTurn))
            .unwrap();
        let records = store
            .list(&SessionId::new("s").unwrap(), None, 100)
            .unwrap();
        let tool = records
            .iter()
            .find_map(|r| {
                if let Payload::Tool(tool) = &r.record.payload {
                    Some(tool)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(tool.input, Some(json!({"path":"a"})));
        assert_eq!(tool.output, Some(json!({"ok":true})));
        assert_eq!(tool.status, ToolStatus::Completed);
        assert!(tool.extensions["acp"].get("content").is_none()); // SDK omits empty collections.
    }

    #[test]
    fn unsupported_content_and_plans_remain_available_as_extensions() {
        let store = MemoryStore::default();
        let mut recorder = recorder(&store);
        recorder.observe(&event(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"YWJj","mimeType":"image/png"}}))).unwrap();
        recorder
            .observe(&event(json!({"sessionUpdate":"plan","entries":[]})))
            .unwrap();
        recorder
            .observe(&AcpEvent::Finished(StopReason::EndTurn))
            .unwrap();
        let records = store
            .list(&SessionId::new("s").unwrap(), None, 100)
            .unwrap();
        assert!(records.iter().any(
            |r| matches!(&r.record.payload, Payload::Extension { name, data, .. }
            if name == "message_content" && data["content"]["data"] == "YWJj")
        ));
        assert!(records.iter().any(
            |r| matches!(&r.record.payload, Payload::Extension { name, data, .. }
            if name == "session_update" && data["sessionUpdate"] == "plan")
        ));
    }
}
