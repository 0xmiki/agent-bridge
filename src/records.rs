//! Portable transcript payloads and a provisional local storage contract.
//!
//! Live records can be checkpointed using revision checks. Finalized records are
//! immutable. This module does not execute tools, resolve context, or send decisions.

mod memory;
pub use memory::MemoryStore;

use crate::{ActorId, Message, Record, RecordId, RunId, RunSpec, SessionId};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageKind {
    User,
    Agent,
    Reasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActivity {
    pub title: String,
    pub status: ToolStatus,
    pub input: Option<Value>,
    pub output: Option<Value>,
    /// Namespaced information that has no portable representation yet.
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    pub label: String,
    /// Preserve provider scope/effect descriptions; never infer authority from labels.
    pub effect: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    Selected(String),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionDelivery {
    /// Accepted by the local transport queue, not acknowledged by the provider.
    Queued,
    /// A decision was attempted, but its delivery is not established.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionReason {
    Completed,
    Refused,
    TokenLimit,
    StepLimit,
    Cancelled,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Message {
        kind: MessageKind,
        message: Message,
    },
    Tool(ToolActivity),
    Permission {
        title: String,
        options: Vec<PermissionOption>,
    },
    Decision {
        outcome: PermissionOutcome,
        delivery: DecisionDelivery,
    },
    Failure {
        message: String,
    },
    RunFinished {
        reason: CompletionReason,
    },
    /// Preserves unsupported SDK data rather than pretending it was normalized.
    Extension {
        namespace: String,
        name: String,
        data: Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordState {
    Open,
    Complete,
    Interrupted,
}

impl RecordState {
    pub fn is_final(self) -> bool {
        self != Self::Open
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub namespace: String,
    pub id: String,
}

/// New record data. The store assigns the session-local sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: RecordId,
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub actor: ActorId,
    pub reply_to_id: Option<RecordId>,
    pub source: Option<SourceRef>,
    pub payload: Payload,
    pub state: RecordState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub record: Record<Payload>,
    pub source: Option<SourceRef>,
    pub revision: u64,
    pub state: RecordState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    MissingSession,
    MissingRun,
    MissingRecord,
    IdentityConflict,
    WrongSession,
    RevisionConflict,
    Finalized,
    InvalidPayload,
    InvalidDecision,
    AlreadyResolved,
    OpenContextRecord,
    InvalidPageSize,
    SequenceExhausted,
    Poisoned,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "record store: {self:?}")
    }
}
impl Error for StoreError {}

/// Local synchronous storage semantics, not a distributed execution scheduler.
/// Implementations must make each mutation atomic. Remote/async adapters and a
/// serialized schema are deliberately not promised by this initial trait.
pub trait RecordStore: Send + Sync {
    fn create_session(&self, id: SessionId) -> Result<(), StoreError>;
    /// Returns true only for a new identity, false for an identical registration.
    fn register_run(&self, spec: RunSpec) -> Result<bool, StoreError>;
    /// Repeating the original draft is idempotent, even after later checkpoints.
    /// Reusing its ID for different data is an error. Decisions use `resolve`.
    fn insert(&self, draft: Draft) -> Result<Arc<Snapshot>, StoreError>;
    /// Compare-and-swap a live payload; identity and ordering never change.
    fn checkpoint(
        &self,
        id: &RecordId,
        expected_revision: u64,
        payload: Payload,
        state: RecordState,
    ) -> Result<Arc<Snapshot>, StoreError>;
    /// Validate and append one decision while finalizing its request atomically.
    /// This records a local decision; it does not send it or execute a tool.
    fn resolve(
        &self,
        request: &RecordId,
        expected_revision: u64,
        decision: Draft,
    ) -> Result<Arc<Snapshot>, StoreError>;
    fn get(&self, id: &RecordId) -> Result<Arc<Snapshot>, StoreError>;
    fn get_run(&self, id: &RunId) -> Result<RunSpec, StoreError>;
    /// `after` is an exclusive session sequence cursor. Limit must be 1..=1000.
    fn list(
        &self,
        session: &SessionId,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Arc<Snapshot>>, StoreError>;
}
