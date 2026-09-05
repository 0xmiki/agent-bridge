use agent_bridge::{ContextManifest, Run, RunEvent, RunId, RunSpec, RunStatus, SessionId, SlotId};

fn queued() -> Run {
    Run::new(RunSpec {
        id: RunId::new("run-1").unwrap(),
        session_id: SessionId::new("session-1").unwrap(),
        slot_id: SlotId::new("slot-1").unwrap(),
        context: ContextManifest::default(),
        config: Default::default(),
        continuation: None,
    })
}

fn running() -> Run {
    let mut run = queued();
    run.apply(RunEvent::DispatchStarted).unwrap();
    run.apply(RunEvent::Started).unwrap();
    run
}

#[test]
fn cancellation_intent_survives_disconnect_and_reconciliation() {
    let mut run = running();
    run.apply(RunEvent::CancellationRequested).unwrap();
    run.apply(RunEvent::ConnectionLost).unwrap();

    assert_eq!(run.status(), RunStatus::Unknown);
    assert!(!run.status().is_terminal());
    assert!(run.cancellation_requested());

    run.apply(RunEvent::RecoveredRunning).unwrap();
    assert_eq!(run.status(), RunStatus::Cancelling);
    run.apply(RunEvent::CancellationConfirmed).unwrap();
    assert_eq!(run.status(), RunStatus::Cancelled);
}

#[test]
fn completion_can_win_a_cancellation_race() {
    let mut run = running();
    run.apply(RunEvent::CancellationRequested).unwrap();
    run.apply(RunEvent::Completed).unwrap();

    assert_eq!(run.status(), RunStatus::Completed);
    assert!(run.cancellation_requested());
}

#[test]
fn cancelling_unknown_work_does_not_claim_it_stopped() {
    let mut run = running();
    run.apply(RunEvent::ConnectionLost).unwrap();
    run.apply(RunEvent::CancellationRequested).unwrap();

    assert_eq!(run.status(), RunStatus::Unknown);
    run.apply(RunEvent::RecoveredRunning).unwrap();
    assert_eq!(run.status(), RunStatus::Cancelling);
}

#[test]
fn queued_cancellation_prevents_starting() {
    let mut run = queued();
    run.apply(RunEvent::CancellationRequested).unwrap();
    let before = run.clone();

    assert!(run.apply(RunEvent::DispatchStarted).is_err());
    assert_eq!(run, before);
    assert_eq!(run.status(), RunStatus::Cancelled);
}

#[test]
fn terminal_outcomes_cannot_be_rewritten_by_late_events() {
    for outcome in [
        RunEvent::Completed,
        RunEvent::Failed,
        RunEvent::CancellationConfirmed,
    ] {
        let mut run = running();
        run.apply(outcome).unwrap();
        let settled = run.clone();

        for event in [
            RunEvent::DispatchStarted,
            RunEvent::Started,
            RunEvent::CancellationRequested,
            RunEvent::Completed,
            RunEvent::Failed,
            RunEvent::CancellationConfirmed,
            RunEvent::ConnectionLost,
            RunEvent::RecoveredRunning,
        ] {
            assert!(run.apply(event).is_err());
            assert_eq!(run, settled);
        }
    }
}

#[test]
fn unstarted_work_cannot_report_success() {
    let mut run = queued();
    assert!(run.apply(RunEvent::Completed).is_err());
    assert_eq!(run.status(), RunStatus::Queued);
    run.apply(RunEvent::Failed).unwrap();
    assert_eq!(run.status(), RunStatus::Failed);
}

#[test]
fn losing_a_dispatch_acknowledgement_does_not_make_retry_safe() {
    let mut run = queued();
    run.apply(RunEvent::DispatchStarted).unwrap();
    run.apply(RunEvent::ConnectionLost).unwrap();

    assert_eq!(run.status(), RunStatus::Unknown);
    assert!(run.apply(RunEvent::DispatchStarted).is_err());
    run.apply(RunEvent::Completed).unwrap();
    assert_eq!(run.status(), RunStatus::Completed);
}

#[test]
fn a_late_start_acknowledgement_preserves_cancellation_intent() {
    let mut run = queued();
    run.apply(RunEvent::DispatchStarted).unwrap();
    run.apply(RunEvent::CancellationRequested).unwrap();
    assert_eq!(run.status(), RunStatus::Cancelling);

    run.apply(RunEvent::Started).unwrap();
    assert_eq!(run.status(), RunStatus::Cancelling);
    assert!(run.cancellation_requested());
}
