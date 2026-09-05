use std::{error::Error, fmt};

use crate::{ContextManifest, ContinuationId, RunConfiguration, RunId, SessionId, SlotId};

/// A caller-prepared assignment, frozen when moved into a run.
///
/// The default configuration captures application selections and the provider's
/// report at dispatch. It does not attest the remote model's actual execution.
/// A continuation identifies the native session's origin, not a per-run re-claim.
/// This type alone does not dispatch work or enforce authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec<C = RunConfiguration> {
    pub id: RunId,
    pub session_id: SessionId,
    pub slot_id: SlotId,
    pub context: ContextManifest,
    pub config: C,
    /// The claimed continuation from which this native session was resumed.
    pub continuation: Option<ContinuationId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Queued,
    /// Dispatch has begun, but execution has not yet been acknowledged.
    Starting,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    /// Execution may still be happening; the connection no longer establishes its outcome.
    Unknown,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Evidence or intent supplied by the owning runtime.
///
/// The caller must serialize dispatch with queued cancellation, confirm provider
/// outcomes, and deduplicate delivered events. This enum does not perform I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEvent {
    /// Must be recorded before sending work to a provider.
    DispatchStarted,
    Started,
    CancellationRequested,
    Completed,
    Failed,
    CancellationConfirmed,
    ConnectionLost,
    /// Provider evidence establishes that the previously unknown run is still active.
    RecoveredRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition {
    pub status: RunStatus,
    pub event: RunEvent,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot apply {:?} to a {:?} run",
            self.event, self.status
        )
    }
}

impl Error for InvalidTransition {}

/// In-memory lifecycle of one bridge-managed execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run<C = RunConfiguration> {
    spec: RunSpec<C>,
    status: RunStatus,
    cancellation_requested: bool,
}

impl<C> Run<C> {
    pub fn new(spec: RunSpec<C>) -> Self {
        Self {
            spec,
            status: RunStatus::Queued,
            cancellation_requested: false,
        }
    }

    pub fn spec(&self) -> &RunSpec<C> {
        &self.spec
    }

    pub fn status(&self) -> RunStatus {
        self.status
    }

    pub fn cancellation_requested(&self) -> bool {
        self.cancellation_requested
    }

    /// Apply one lifecycle fact. Rejected events leave the run unchanged.
    ///
    /// Cancellation intent is retained across disconnection. Completing after a
    /// cancellation request is valid; losing the connection is never completion.
    pub fn apply(&mut self, event: RunEvent) -> Result<RunStatus, InvalidTransition> {
        use RunEvent as E;
        use RunStatus as S;

        let next = match (self.status, event) {
            (S::Queued, E::DispatchStarted) => S::Starting,
            (S::Starting, E::Started) => S::Running,
            // A late start acknowledgement must not erase cancellation intent.
            (S::Cancelling, E::Started) => S::Cancelling,
            (S::Queued, E::CancellationRequested) => S::Cancelled,
            (S::Queued | S::Starting | S::Running | S::Cancelling | S::Unknown, E::Failed) => {
                S::Failed
            }
            (S::Starting | S::Running | S::Cancelling, E::CancellationRequested) => S::Cancelling,
            (S::Unknown, E::CancellationRequested) => S::Unknown,
            (S::Starting | S::Running | S::Cancelling | S::Unknown, E::Completed) => S::Completed,
            (S::Starting | S::Running | S::Cancelling | S::Unknown, E::CancellationConfirmed) => {
                S::Cancelled
            }
            (S::Starting | S::Running | S::Cancelling, E::ConnectionLost) => S::Unknown,
            (S::Unknown, E::RecoveredRunning) if self.cancellation_requested => S::Cancelling,
            (S::Unknown, E::RecoveredRunning) => S::Running,
            _ => {
                return Err(InvalidTransition {
                    status: self.status,
                    event,
                });
            }
        };

        if event == E::CancellationRequested {
            self.cancellation_requested = true;
        }
        self.status = next;
        Ok(next)
    }
}
