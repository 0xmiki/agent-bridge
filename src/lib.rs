//! Shared integration for connecting applications to installed AI agents.
//!
//! The execution model is independent of transports. The optional `acp` module adds
//! subprocess sessions, streamed text runs, and permission routing. The `records`
//! feature adds portable transcripts and an in-memory store. The optional `sqlite`
//! feature persists records and single-use continuation handles across restarts.
//! The optional `providers` feature adds installed-provider discovery and drivers.
//! Provider execution recovery is still separate work.
//! Public types remain provisional.

#[cfg(feature = "acp")]
pub mod acp;

#[cfg(feature = "providers")]
pub mod providers;

#[cfg(feature = "records")]
pub mod records;

#[cfg(feature = "records")]
pub mod context;

#[cfg(feature = "structured")]
pub mod structured;

mod configuration;
mod id;
mod model;
mod run;
pub use configuration::{
    ConfigChoice, ConfigOption, ConfigValue, ConfigValues, RunConfiguration, SessionConfiguration,
};

pub use id::{ActorId, ContinuationId, InvalidId, RecordId, ResourceId, RunId, SessionId, SlotId};
pub use model::{
    Content, ContextManifest, InstructionRef, InstructionRole, Message, Record, ResourceRef,
    Session, Slot,
};
pub use run::{InvalidTransition, Run, RunEvent, RunSpec, RunStatus};
