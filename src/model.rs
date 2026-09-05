use crate::{ActorId, RecordId, ResourceId, RunId, SessionId, SlotId};

/// Configured execution capacity; not a participant or a live process.
///
/// `C` is caller-defined configuration until provider contracts are established.
/// Configuration must not contain credentials intended for persistent storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot<C = ()> {
    pub id: SlotId,
    pub driver: String,
    pub config: C,
}

/// Application context identity, independent of provider sessions and slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
}

/// A specific resource revision. Resolution and retention belong to its store.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct ResourceRef {
    pub id: ResourceId,
    pub revision: String,
}

/// Intended instruction semantics, not a guarantee of provider support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(rename_all = "snake_case"))]
pub enum InstructionRole {
    /// Requires the provider's base-instruction mechanism.
    Base,
    /// Additional guidance that does not replace base instructions.
    Supplemental,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct InstructionRef {
    pub resource: ResourceRef,
    pub role: InstructionRole,
}

/// Ordered selections supplied by the application, not the provider's hidden context.
///
/// Referenced records must be immutable once used by a run. Stores and adapters
/// must validate access, existence, and supported semantics before dispatch.
/// An empty manifest selects no explicit context; it does not disable provider defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct ContextManifest {
    pub records: Vec<RecordId>,
    pub instructions: Vec<InstructionRef>,
    pub resources: Vec<ResourceRef>,
}

/// An attributed record. The payload remains typed without fixing a wire format.
///
/// Storage assigns unique, increasing sequences within a session and validates
/// run/session relationships. `run_id` is absent for input outside a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record<P> {
    pub id: RecordId,
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub sequence: u64,
    pub actor: ActorId,
    pub reply_to_id: Option<RecordId>,
    pub payload: P,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct Message {
    pub content: Vec<Content>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "sqlite",
    serde(
        tag = "type",
        content = "data",
        rename_all = "snake_case",
        deny_unknown_fields
    )
)]
pub enum Content {
    Text(String),
    Resource(ResourceRef),
}
