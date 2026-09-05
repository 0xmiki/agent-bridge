#![cfg(feature = "records")]
use agent_bridge::records::*;
use agent_bridge::{
    ActorId, Content, ContextManifest, Message, RecordId, RunId, RunSpec, SessionId, SlotId,
};

fn spec(id: &str, session: &str) -> RunSpec {
    RunSpec {
        id: RunId::new(id).unwrap(),
        session_id: SessionId::new(session).unwrap(),
        slot_id: SlotId::new("slot").unwrap(),
        context: ContextManifest::default(),
        config: (),
    }
}
fn store() -> MemoryStore {
    let store = MemoryStore::default();
    store.create_session(SessionId::new("s").unwrap()).unwrap();
    store.register_run(spec("r", "s")).unwrap();
    store
}
fn message(text: &str) -> Payload {
    Payload::Message {
        kind: MessageKind::Agent,
        message: Message {
            content: vec![Content::Text(text.into())],
        },
    }
}
fn draft(id: &str, payload: Payload, state: RecordState) -> Draft {
    Draft {
        id: RecordId::new(id).unwrap(),
        session_id: SessionId::new("s").unwrap(),
        run_id: Some(RunId::new("r").unwrap()),
        actor: ActorId::new("reviewer").unwrap(),
        reply_to_id: None,
        source: None,
        payload,
        state,
    }
}
fn permission() -> Draft {
    draft(
        "permission",
        Payload::Permission {
            title: "Read?".into(),
            options: vec![
                PermissionOption {
                    id: "allow".into(),
                    label: "Allow once".into(),
                    effect: "allow_once".into(),
                },
                PermissionOption {
                    id: "reject".into(),
                    label: "Reject once".into(),
                    effect: "reject_once".into(),
                },
            ],
        },
        RecordState::Open,
    )
}
fn decision(id: &str, choice: &str) -> Draft {
    let mut draft = draft(
        id,
        Payload::Decision {
            outcome: PermissionOutcome::Selected(choice.into()),
            delivery: DecisionDelivery::Queued,
        },
        RecordState::Complete,
    );
    draft.reply_to_id = Some(RecordId::new("permission").unwrap());
    draft
}

#[test]
fn retried_creation_preserves_identity_order_and_latest_checkpoint() {
    let store = store();
    let draft = draft("m", message(""), RecordState::Open);
    let initial = store.insert(draft.clone()).unwrap();
    let final_record = store
        .checkpoint(
            &initial.record.id,
            0,
            message("Hello"),
            RecordState::Complete,
        )
        .unwrap();
    let retried = store.insert(draft).unwrap();
    assert_eq!(retried, final_record);
    assert_eq!(retried.record.sequence, 0);
    assert_eq!(
        store
            .list(&SessionId::new("s").unwrap(), None, 100)
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        store.insert(self::draft("m", message("other"), RecordState::Open)),
        Err(StoreError::IdentityConflict)
    ));
}

#[test]
fn checkpoints_reject_stale_writers_and_finalized_mutations() {
    let store = store();
    let id = store
        .insert(draft("m", message(""), RecordState::Open))
        .unwrap()
        .record
        .id
        .clone();
    store
        .checkpoint(&id, 0, message("a"), RecordState::Open)
        .unwrap();
    assert!(matches!(
        store.checkpoint(&id, 0, message("b"), RecordState::Open),
        Err(StoreError::RevisionConflict)
    ));
    let sealed = store
        .checkpoint(&id, 1, message("abc"), RecordState::Complete)
        .unwrap();
    assert_eq!(
        store
            .checkpoint(&id, 1, message("abc"), RecordState::Complete)
            .unwrap(),
        sealed
    );
    assert!(matches!(
        store.checkpoint(&id, 2, message("changed"), RecordState::Complete),
        Err(StoreError::Finalized)
    ));
    assert_eq!(store.get(&id).unwrap(), sealed);
}

#[test]
fn validates_ownership_and_finalized_context_references() {
    let store = store();
    let open = store
        .insert(draft("m", message("partial"), RecordState::Open))
        .unwrap();
    let mut next = spec("next", "s");
    next.context.records.push(open.record.id.clone());
    assert_eq!(
        store.register_run(next.clone()),
        Err(StoreError::OpenContextRecord)
    );
    store
        .checkpoint(
            &open.record.id,
            0,
            message("partial"),
            RecordState::Interrupted,
        )
        .unwrap();
    assert_eq!(store.register_run(next.clone()), Ok(true));
    assert_eq!(store.register_run(next), Ok(false));
    store
        .create_session(SessionId::new("other").unwrap())
        .unwrap();
    let mut wrong = draft("wrong", message("x"), RecordState::Complete);
    wrong.session_id = SessionId::new("other").unwrap();
    assert!(matches!(store.insert(wrong), Err(StoreError::WrongSession)));
    let mut missing = draft("missing", message("x"), RecordState::Complete);
    missing.run_id = Some(RunId::new("missing").unwrap());
    assert!(matches!(store.insert(missing), Err(StoreError::MissingRun)));
}

#[test]
fn pagination_is_exclusive_and_isolated_by_session() {
    let store = store();
    for id in ["a", "b", "c"] {
        store
            .insert(draft(id, message(id), RecordState::Complete))
            .unwrap();
    }
    let id = SessionId::new("s").unwrap();
    assert_eq!(
        store
            .list(&id, None, 2)
            .unwrap()
            .iter()
            .map(|s| s.record.sequence)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(store.list(&id, Some(1), 2).unwrap()[0].record.sequence, 2);
    assert!(store.list(&id, Some(u64::MAX), 2).unwrap().is_empty());
    assert!(matches!(
        store.list(&id, None, 0),
        Err(StoreError::InvalidPageSize)
    ));
    store
        .create_session(SessionId::new("empty").unwrap())
        .unwrap();
    assert!(
        store
            .list(&SessionId::new("empty").unwrap(), None, 100)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn invalid_decisions_are_atomic_and_valid_retries_are_idempotent() {
    let store = store();
    let request = store.insert(permission()).unwrap();
    assert!(matches!(
        store.resolve(&request.record.id, 0, decision("response", "invented")),
        Err(StoreError::InvalidDecision)
    ));
    assert_eq!(store.get(&request.record.id).unwrap(), request);
    assert!(matches!(
        store.get(&RecordId::new("response").unwrap()),
        Err(StoreError::MissingRecord)
    ));
    assert!(matches!(
        store.insert(decision("bypass", "allow")),
        Err(StoreError::InvalidDecision)
    ));
    let response = store
        .resolve(&request.record.id, 0, decision("response", "allow"))
        .unwrap();
    assert_eq!(
        store
            .resolve(&request.record.id, 0, decision("response", "allow"))
            .unwrap(),
        response
    );
    assert_eq!(
        store.get(&request.record.id).unwrap().state,
        RecordState::Complete
    );
    assert!(matches!(
        store.resolve(&request.record.id, 1, decision("other", "reject")),
        Err(StoreError::AlreadyResolved)
    ));
    assert_eq!(
        store
            .list(&request.record.session_id, None, 100)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn concurrent_decisions_have_exactly_one_winner() {
    let store = store();
    let request = store.insert(permission()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = ["allow", "reject"]
        .into_iter()
        .map(|choice| {
            let store = store.clone();
            let request = request.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.resolve(&request.record.id, 0, decision(choice, choice))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        store
            .list(&request.record.session_id, None, 100)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn concurrent_inserts_get_unique_session_sequences() {
    let store = store();
    let threads: Vec<_> = (0..16)
        .map(|index| {
            let store = store.clone();
            std::thread::spawn(move || {
                store
                    .insert(draft(
                        &index.to_string(),
                        message("x"),
                        RecordState::Complete,
                    ))
                    .unwrap()
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let sequences: Vec<_> = store
        .list(&SessionId::new("s").unwrap(), None, 100)
        .unwrap()
        .iter()
        .map(|s| s.record.sequence)
        .collect();
    assert_eq!(sequences, (0..16).collect::<Vec<_>>());
}
