//! Local awaitable questions. Storage resolution and provider delivery are separate.
use crate::records::{
    AnswerDelivery, AnswerOutcome, Draft, Payload, RecordState, RecordStore, Snapshot, StoreError,
};
use crate::{ActorId, RecordId};
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;
pub use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub enum QuestionError {
    Store(StoreError),
    NotQuestion,
    AlreadyExists,
    ReceiverClosed,
}
impl fmt::Display for QuestionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "question: {self:?}")
    }
}
impl Error for QuestionError {}
impl From<StoreError> for QuestionError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

struct Gate {
    sender: Option<oneshot::Sender<Arc<Snapshot>>>,
    resolved: Option<Arc<Snapshot>>,
}
struct Shared<S: RecordStore + ?Sized> {
    store: Arc<S>,
    request: Arc<Snapshot>,
    host: ActorId,
    gate: Mutex<Gate>,
}

pub struct QuestionResponder<S: RecordStore + ?Sized> {
    shared: Arc<Shared<S>>,
}
impl<S: RecordStore + ?Sized> Clone for QuestionResponder<S> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}
impl<S: RecordStore + ?Sized> QuestionResponder<S> {
    pub fn question(&self) -> &Arc<Snapshot> {
        &self.shared.request
    }

    pub fn answer(
        &self,
        actor: ActorId,
        outcome: AnswerOutcome,
    ) -> Result<Arc<Snapshot>, QuestionError> {
        self.resolve(actor, outcome, false)
    }
    fn resolve(
        &self,
        actor: ActorId,
        outcome: AnswerOutcome,
        accept_existing: bool,
    ) -> Result<Arc<Snapshot>, QuestionError> {
        let mut gate = self.shared.gate.lock().map_err(|_| StoreError::Poisoned)?;
        if let Some(existing) = &gate.resolved {
            return if accept_existing
                || (existing.record.actor == actor
                    && existing.record.payload
                        == (Payload::Answer {
                            outcome: outcome.clone(),
                            delivery: AnswerDelivery::Stored,
                        }))
            {
                Ok(existing.clone())
            } else {
                Err(StoreError::AlreadyResolved.into())
            };
        }
        let request = &self.shared.request;
        let raw_id = request.record.id.as_str();
        let answer = self.shared.store.resolve(
            &request.record.id,
            request.revision,
            Draft {
                id: RecordId::new(format!("question/{}:{raw_id}/answer", raw_id.len())).unwrap(),
                session_id: request.record.session_id.clone(),
                run_id: request.record.run_id.clone(),
                actor,
                reply_to_id: Some(request.record.id.clone()),
                source: None,
                state: RecordState::Complete,
                payload: Payload::Answer {
                    outcome,
                    delivery: AnswerDelivery::Stored,
                },
            },
        )?;
        gate.resolved = Some(answer.clone());
        if let Some(sender) = gate.sender.take() {
            let _ = sender.send(answer.clone());
        }
        Ok(answer)
    }
    fn cancel(&self) -> Result<Arc<Snapshot>, QuestionError> {
        self.resolve(self.shared.host.clone(), AnswerOutcome::Cancelled, true)
    }
}

pub struct PendingQuestion<S: RecordStore + ?Sized> {
    responder: QuestionResponder<S>,
    receiver: oneshot::Receiver<Arc<Snapshot>>,
}
impl<S: RecordStore + ?Sized> PendingQuestion<S> {
    /// Persist before handing the responder to a UI or other host component.
    /// Callers allocate unique request IDs; this is not a distributed waiter claim.
    pub fn open(store: Arc<S>, request: Draft, host: ActorId) -> Result<Self, QuestionError> {
        if !matches!(request.payload, Payload::Question(_)) || request.state != RecordState::Open {
            return Err(QuestionError::NotQuestion);
        }
        match store.get(&request.id) {
            Ok(_) => return Err(QuestionError::AlreadyExists),
            Err(StoreError::MissingRecord) => {}
            Err(error) => return Err(error.into()),
        }
        let request = store.insert(request)?;
        if request.state != RecordState::Open {
            return Err(StoreError::Finalized.into());
        }
        let (sender, receiver) = oneshot::channel();
        Ok(Self {
            responder: QuestionResponder {
                shared: Arc::new(Shared {
                    store,
                    request,
                    host,
                    gate: Mutex::new(Gate {
                        sender: Some(sender),
                        resolved: None,
                    }),
                }),
            },
            receiver,
        })
    }
    pub fn responder(&self) -> QuestionResponder<S> {
        self.responder.clone()
    }
    pub async fn wait(
        mut self,
        cancellation: CancellationToken,
    ) -> Result<Arc<Snapshot>, QuestionError> {
        tokio::select! {
            biased;
            answer = &mut self.receiver => answer.map_err(|_| QuestionError::ReceiverClosed),
            _ = cancellation.cancelled() => self.responder.cancel(),
        }
    }
}
impl<S: RecordStore + ?Sized> Drop for PendingQuestion<S> {
    fn drop(&mut self) {
        let _ = self.responder.cancel();
    }
}
