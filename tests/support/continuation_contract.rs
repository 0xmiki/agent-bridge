use agent_bridge::records::{Continuation, ContinuationState, StoreError};
use agent_bridge::{ContinuationId, SessionId, SlotId};
use serde_json::json;

fn descriptor(id: &str) -> Continuation {
    Continuation {
        id: ContinuationId::new(id).unwrap(),
        session_id: SessionId::new("s").unwrap(),
        slot_id: SlotId::new("slot").unwrap(),
        adapter: "test".into(),
        scope: "account-a".into(),
        native_key: "native".into(),
        predecessor: None,
        data: json!({"version":1}),
    }
}

#[test]
fn claims_are_single_use_and_insertion_retries_do_not_reactivate_them() {
    let store = fresh_store();
    store.create_session(SessionId::new("s").unwrap()).unwrap();
    let descriptor = descriptor("first");
    store.save_continuation(descriptor.clone()).unwrap();
    let claimed = store.claim_continuation(&descriptor.id).unwrap();
    assert_eq!(claimed.state, ContinuationState::Claimed);
    assert_eq!(
        store.save_continuation(descriptor.clone()).unwrap(),
        claimed
    );
    assert_eq!(
        store.claim_continuation(&descriptor.id),
        Err(StoreError::ContinuationClaimed)
    );
}

#[test]
fn one_claimed_handoff_can_have_only_one_successor() {
    let store = fresh_store();
    store.create_session(SessionId::new("s").unwrap()).unwrap();
    let first = descriptor("first");
    store.save_continuation(first.clone()).unwrap();
    let mut next = descriptor("next");
    next.predecessor = Some(first.id.clone());
    assert_eq!(
        store.save_continuation(next.clone()),
        Err(StoreError::ContinuationConflict)
    );
    assert_eq!(
        store.save_continuation(descriptor("alias")),
        Err(StoreError::ContinuationConflict)
    );
    store.claim_continuation(&first.id).unwrap();
    store.save_continuation(next.clone()).unwrap();
    assert!(!store.get_continuation(&first.id).unwrap().latest);
    let mut competing = next.clone();
    competing.id = ContinuationId::new("competing").unwrap();
    assert_eq!(
        store.save_continuation(competing),
        Err(StoreError::ContinuationConflict)
    );
    assert_eq!(
        store.get_continuation(&next.id).unwrap().state,
        ContinuationState::Available
    );
}

#[test]
fn concurrent_claims_have_one_winner() {
    let store = fresh_store();
    store.create_session(SessionId::new("s").unwrap()).unwrap();
    let descriptor = descriptor("race");
    store.save_continuation(descriptor.clone()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            let id = descriptor.id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.claim_continuation(&id)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::ContinuationClaimed)))
            .count(),
        1
    );
}

#[test]
fn descriptors_are_immutable_and_bound_to_existing_sessions() {
    let store = fresh_store();
    let descriptor = descriptor("one");
    assert_eq!(
        store.save_continuation(descriptor.clone()),
        Err(StoreError::MissingSession)
    );
    store.create_session(descriptor.session_id.clone()).unwrap();
    store.save_continuation(descriptor.clone()).unwrap();
    let mut different = descriptor.clone();
    different.data = json!({"version":2});
    assert_eq!(
        store.save_continuation(different),
        Err(StoreError::IdentityConflict)
    );
    assert_eq!(
        store.get_continuation(&ContinuationId::new("missing").unwrap()),
        Err(StoreError::MissingContinuation)
    );
}

#[test]
fn runs_link_only_to_claimed_current_continuations_with_matching_ownership() {
    use agent_bridge::{ContextManifest, RunId, RunSpec};
    let store = fresh_store();
    let descriptor = descriptor("origin");
    store.create_session(descriptor.session_id.clone()).unwrap();
    store.save_continuation(descriptor.clone()).unwrap();
    let run = RunSpec {
        id: RunId::new("continued").unwrap(),
        session_id: descriptor.session_id.clone(),
        slot_id: descriptor.slot_id.clone(),
        context: ContextManifest::default(),
        config: Default::default(),
        continuation: Some(descriptor.id.clone()),
    };
    assert_eq!(
        store.register_run(run.clone()),
        Err(StoreError::InvalidContinuation)
    );
    store.claim_continuation(&descriptor.id).unwrap();
    let mut wrong = run.clone();
    wrong.slot_id = SlotId::new("other").unwrap();
    assert_eq!(
        store.register_run(wrong),
        Err(StoreError::InvalidContinuation)
    );
    assert_eq!(store.register_run(run.clone()), Ok(true));
    assert_eq!(
        store.get_run(&run.id).unwrap().continuation,
        Some(descriptor.id.clone())
    );
    let mut successor = descriptor.clone();
    successor.id = ContinuationId::new("successor").unwrap();
    successor.predecessor = Some(descriptor.id);
    store.save_continuation(successor).unwrap();
    assert_eq!(store.get_run(&run.id).unwrap(), run);
    let mut stale = run;
    stale.id = RunId::new("stale").unwrap();
    assert_eq!(
        store.register_run(stale),
        Err(StoreError::InvalidContinuation)
    );
}
