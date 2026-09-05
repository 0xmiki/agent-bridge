//! Shared integration for connecting applications to installed AI agents.
//!
//! This first slice defines an execution model. It does not connect to providers
//! or persist data yet. Public types remain provisional.

mod id;
mod model;
mod run;

pub use id::{ActorId, InvalidId, RecordId, ResourceId, RunId, SessionId, SlotId};
pub use model::{
    Content, ContextManifest, InstructionRef, InstructionRole, Message, Record, ResourceRef,
    Session, Slot,
};
pub use run::{InvalidTransition, Run, RunEvent, RunSpec, RunStatus};
