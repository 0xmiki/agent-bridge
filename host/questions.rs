use super::{Output, Request, identity, tool_hub::QuestionOwner};
use agent_bridge::questions::{PendingQuestion, QuestionResponder};
use agent_bridge::records::{
    AnswerOutcome, Draft, Payload, Question, QuestionFieldKind, RecordState, SourceRef,
};
use agent_bridge::{ActorId, RecordId};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Semaphore;

struct Entry {
    owner: QuestionOwner,
    responder: QuestionResponder<super::storage::Store>,
    resolved: AtomicBool,
}
impl Entry {
    fn frame(&self) -> Value {
        let request = self.responder.question();
        let Payload::Question(question) = &request.record.payload else {
            unreachable!()
        };
        json!({"question_id":request.record.id.as_str(),"revision":request.revision.to_string(),"call_id":self.owner.call,
            "binding_id":self.owner.binding,"session_id":self.owner.scope.session.as_str(),"slot_id":self.owner.scope.slot.as_str(),"definition":question})
    }
    fn matches(&self, params: &Value) -> bool {
        params["call_id"].as_str() == Some(&self.owner.call)
            && params["binding_id"].as_str() == Some(&self.owner.binding)
            && params["session_id"].as_str() == Some(self.owner.scope.session.as_str())
            && params["slot_id"].as_str() == Some(self.owner.scope.slot.as_str())
            && params["revision"].as_str()
                == Some(self.responder.question().revision.to_string().as_str())
            && !self.owner.cancellation.is_cancelled()
    }
}
pub struct Service {
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    output: Output,
    operations: Arc<Semaphore>,
}
impl Service {
    pub fn new(output: Output) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            output,
            operations: Arc::new(Semaphore::new(32)),
        }
    }
    pub fn dispatch(
        self: &Arc<Self>,
        runtime: &tokio::runtime::Handle,
        request: Request,
        owner: Option<QuestionOwner>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let permit = match self.operations.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                self.output.error(
                    &request.id,
                    "question_capacity",
                    "too many question operations",
                );
                return None;
            }
        };
        let service = self.clone();
        Some(runtime.spawn(async move {
            if request.method == "question_ask" {
                let owner=owner.expect("validated question owner");
                let opened=service.open(owner.clone(),&request.params);
                let (entry, pending)=match opened {Ok(opened)=>opened,Err(error)=>{service.output.error(&request.id,"invalid_question",error);return;}};
                let id=entry.responder.question().record.id.as_str().to_owned();
                let mut frame=entry.frame();frame["event"]=json!("question_opened");service.output.emit(frame);
                let cancel=owner.cancellation.child_token();
                let result=tokio::select! {
                    biased;
                    answer=pending.wait(cancel.clone())=>answer,
                    _=service.output.stop.cancelled()=>{cancel.cancel();return;}
                };
                entry.resolved.store(true,Ordering::SeqCst);
                service.output.emit(json!({"event":"question_closed","question_id":id,"call_id":owner.call,"binding_id":owner.binding,"session_id":owner.scope.session.as_str(),"slot_id":owner.scope.slot.as_str()}));
                match result {
                    Ok(answer)=>{let Payload::Answer {outcome,delivery}=&answer.record.payload else {unreachable!()};service.output.ok(&request.id,json!({"question_id":id,"outcome":outcome,"delivery":delivery,"answer_record_id":answer.record.id.as_str(),"invocation_active":!owner.cancellation.is_cancelled() && !service.output.stop.is_cancelled()}));},
                    Err(error)=>service.output.error(&request.id,"question_failed",error),
                }
                drop(permit);
                // Keep resolved responders only during this live invocation so
                // identical answers are idempotent without an unbounded cache.
                tokio::select! {_=owner.cancellation.cancelled()=>{},_=service.output.stop.cancelled()=>{}}
                service.entries.lock().unwrap().remove(&id);
            } else {
                let _permit=permit;
                let result=if request.method=="question_answer" {service.answer(&request.params)} else {Ok(service.pending(&request.params))};
                match result {Ok(value)=>service.output.ok(&request.id,value),Err(error)=>service.output.error(&request.id,"invalid_answer",error)}
            }
        }))
    }
    fn open(
        &self,
        owner: QuestionOwner,
        params: &Value,
    ) -> Result<(Arc<Entry>, PendingQuestion<super::storage::Store>), String> {
        let definition = &params["definition"];
        if definition.to_string().len() > 16384 || !super::tool_hub::valid_json(definition, 0) {
            return Err("question exceeds JSON limits".into());
        }
        let question: Question =
            serde_json::from_value(definition.clone()).map_err(|e| e.to_string())?;
        question.validate().map_err(|e| e.to_string())?;
        if question.fields.iter().any(|f| matches!(f.kind,QuestionFieldKind::Text{max_bytes} if max_bytes>16384) || matches!(f.kind,QuestionFieldKind::Integer{min,max} if min < -9_007_199_254_740_991 || max > 9_007_199_254_740_991)) {return Err("question field exceeds transport bounds".into());}
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, entry| !entry.owner.cancellation.is_cancelled());
        if owner.cancellation.is_cancelled() || self.output.stop.is_cancelled() {
            return Err("invocation ended".into());
        }
        let same: Vec<_> = entries
            .values()
            .filter(|e| e.owner.call == owner.call)
            .collect();
        if entries.len() >= 128
            || same.len() >= 8
            || same.iter().any(|e| !e.resolved.load(Ordering::SeqCst))
        {
            return Err(
                "question limit reached or invocation already has a pending question".into(),
            );
        }
        let id = identity("question");
        let pending = PendingQuestion::open(
            owner.store.clone(),
            Draft {
                id: RecordId::new(&id).unwrap(),
                session_id: owner.scope.session.clone(),
                run_id: None,
                actor: ActorId::new("host").unwrap(),
                reply_to_id: None,
                source: Some(SourceRef {
                    namespace: "agent_bridge.tool_invocation".into(),
                    id: owner.call.clone(),
                }),
                state: RecordState::Open,
                payload: Payload::Question(question),
            },
            ActorId::new("host").unwrap(),
        )
        .map_err(|e| e.to_string())?;
        let entry = Arc::new(Entry {
            owner,
            responder: pending.responder(),
            resolved: AtomicBool::new(false),
        });
        entries.insert(id, entry.clone());
        Ok((entry, pending))
    }
    fn answer(&self, params: &Value) -> Result<Value, String> {
        let id = super::string(params, "question_id")?;
        let entry = self
            .entries
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("question is no longer live")?;
        if !entry.matches(params) {
            return Err("stale or foreign question response".into());
        }
        let value = &params["outcome"];
        if value.to_string().len() > 60000 || !super::tool_hub::valid_json(value, 0) {
            return Err("answer exceeds JSON limits".into());
        }
        let outcome: AnswerOutcome =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let answer = entry
            .responder
            .answer(ActorId::new("user").unwrap(), outcome)
            .map_err(|e| e.to_string())?;
        entry.resolved.store(true, Ordering::SeqCst);
        Ok(json!({"question_id":id,"answer_record_id":answer.record.id.as_str(),"stored":true}))
    }
    fn pending(&self, params: &Value) -> Value {
        let entries = self.entries.lock().unwrap();
        let mut values: Vec<_> = entries
            .values()
            .filter(|e| {
                !e.resolved.load(Ordering::SeqCst)
                    && !e.owner.cancellation.is_cancelled()
                    && params
                        .get("session_id")
                        .is_none_or(|id| id.as_str() == Some(e.owner.scope.session.as_str()))
            })
            .map(|e| e.frame())
            .collect();
        values.sort_by(|a, b| a["question_id"].as_str().cmp(&b["question_id"].as_str()));
        json!(values)
    }
}
