//! Shared integration for connecting applications to installed AI agents.
//!
//! The execution model is independent of transports. The optional `acp` module adds
//! subprocess sessions, streamed text runs, and permission routing. Persistence
//! and portable record conversion are still to come.
//! Public types remain provisional.

#[cfg(feature = "acp")]
pub mod acp;

mod id;
mod model;
mod run;

pub use id::{ActorId, InvalidId, RecordId, ResourceId, RunId, SessionId, SlotId};
pub use model::{
    Content, ContextManifest, InstructionRef, InstructionRole, Message, Record, ResourceRef,
    Session, Slot,
};
pub use run::{InvalidTransition, Run, RunEvent, RunSpec, RunStatus};
