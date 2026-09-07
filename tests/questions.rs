#![cfg(feature = "records")]
use agent_bridge::records::*;
use agent_bridge::{ActorId, RecordId, SessionId};
use std::{collections::BTreeMap, sync::Arc};

fn question() -> Question {
    Question {
        title: "Choose a destination".into(),
        fields: vec![QuestionField {
            id: "destination".into(),
            label: "Destination".into(),
            required: true,
            kind: QuestionFieldKind::Select {
                options: vec![
                    QuestionOption {
                        id: "draft".into(),
                        label: "Draft".into(),
                    },
                    QuestionOption {
                        id: "review".into(),
                        label: "Review".into(),
                    },
                ],
            },
        }],
    }
}
fn request() -> Draft {
    Draft {
        id: RecordId::new("question").unwrap(),
        session_id: SessionId::new("session").unwrap(),
        run_id: None,
        actor: ActorId::new("agent").unwrap(),
        reply_to_id: None,
        source: None,
        payload: Payload::Question(question()),
        state: RecordState::Open,
    }
}
fn response(outcome: AnswerOutcome) -> Draft {
    Draft {
        id: RecordId::new("answer").unwrap(),
        reply_to_id: Some(RecordId::new("question").unwrap()),
        actor: ActorId::new("person").unwrap(),
        payload: Payload::Answer {
            outcome,
            delivery: AnswerDelivery::Stored,
        },
        state: RecordState::Complete,
        ..request()
    }
}
fn selected(id: &str) -> AnswerOutcome {
    AnswerOutcome::Submitted(BTreeMap::from([(
        "destination".into(),
        AnswerValue::Selected(id.into()),
    )]))
}

fn contract(store: Arc<dyn RecordStore>) {
    store
        .create_session(SessionId::new("session").unwrap())
        .unwrap();
    let opened = store.insert(request()).unwrap();
    assert!(matches!(
        store.insert(response(selected("draft"))),
        Err(StoreError::InvalidDecision)
    ));
    assert!(matches!(
        store.checkpoint(
            &opened.record.id,
            0,
            opened.record.payload.clone(),
            RecordState::Complete
        ),
        Err(StoreError::InvalidDecision)
    ));
    for outcome in [
        selected("unknown"),
        AnswerOutcome::Submitted(BTreeMap::new()),
        AnswerOutcome::Submitted(BTreeMap::from([(
            "destination".into(),
            AnswerValue::Text("draft".into()),
        )])),
    ] {
        assert!(matches!(
            store.resolve(&opened.record.id, 0, response(outcome)),
            Err(StoreError::InvalidDecision)
        ));
    }
    assert_eq!(
        store.get(&opened.record.id).unwrap().state,
        RecordState::Open
    );
    let mut permission = response(selected("draft"));
    permission.payload = Payload::Decision {
        outcome: PermissionOutcome::Selected("draft".into()),
        delivery: DecisionDelivery::Queued,
    };
    assert!(matches!(
        store.resolve(&opened.record.id, 0, permission),
        Err(StoreError::InvalidDecision)
    ));
    let answer = response(selected("review"));
    let saved = store.resolve(&opened.record.id, 0, answer.clone()).unwrap();
    assert_eq!(
        *store.resolve(&opened.record.id, 0, answer).unwrap(),
        *saved
    );
    assert_eq!(
        store.get(&opened.record.id).unwrap().state,
        RecordState::Complete
    );
    assert!(matches!(
        store.resolve(&opened.record.id, 0, response(AnswerOutcome::Declined)),
        Err(StoreError::AlreadyResolved)
    ));
}
#[test]
fn memory_question_resolution_is_validated_atomic_and_idempotent() {
    contract(Arc::new(MemoryStore::default()));
}
#[cfg(feature = "sqlite")]
#[test]
fn sqlite_question_resolution_is_validated_atomic_and_idempotent() {
    contract(Arc::new(SqliteStore::open_in_memory().unwrap()));
}

#[cfg(feature = "sqlite")]
#[test]
fn v4_upgrade_preserves_legacy_records_and_new_answers_survive_reopen() {
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("question-upgrade-{}.sqlite3", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let sql = rusqlite::Connection::open(&path).unwrap();
    for migration in [
        include_str!("../src/records/sqlite/migrations/0001_records.sql"),
        include_str!("../src/records/sqlite/migrations/0002_continuations.sql"),
        include_str!("../src/records/sqlite/migrations/0003_run_configuration.sql"),
        include_str!("../src/records/sqlite/migrations/0004_resources.sql"),
    ] {
        sql.execute_batch(migration).unwrap();
    }
    sql.execute_batch("UPDATE agent_bridge_schema SET version=4; INSERT INTO agent_bridge_sessions(id,next_sequence) VALUES ('session',1);").unwrap();
    sql.execute("INSERT INTO agent_bridge_records(id,session_id,sequence,actor_id,payload_json,state,revision) VALUES ('legacy','session',0,'person',?1,'complete',0)",
        [serde_json::json!({"version":1,"data":{"type":"message","data":{"kind":"user","message":{"content":[{"type":"text","data":"preserved"}]}}}}).to_string()]).unwrap();
    drop(sql);
    let store = SqliteStore::open(&path).unwrap();
    assert!(matches!(
        store
            .get(&RecordId::new("legacy").unwrap())
            .unwrap()
            .record
            .payload,
        Payload::Message { .. }
    ));
    let opened = store.insert(request()).unwrap();
    store
        .resolve(&opened.record.id, 0, response(selected("review")))
        .unwrap();
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .get(&RecordId::new("question").unwrap())
            .unwrap()
            .state,
        RecordState::Complete
    );
    assert_eq!(
        reopened
            .get(&RecordId::new("answer").unwrap())
            .unwrap()
            .record
            .payload,
        response(selected("review")).payload
    );
    let sql = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        sql.query_row("SELECT version FROM agent_bridge_schema", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(sql.query_row("SELECT json_extract(payload_json,'$.version') FROM agent_bridge_records WHERE id='answer'", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(sql.query_row("SELECT json_extract(payload_json,'$.version') FROM agent_bridge_records WHERE id='legacy'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    drop(sql);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[cfg(all(feature = "questions", feature = "sqlite"))]
#[tokio::test]
async fn failed_answer_write_does_not_finalize_or_resume_the_question() {
    use agent_bridge::questions::{CancellationToken, PendingQuestion};
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("question-failure-{}.sqlite3", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    store
        .create_session(SessionId::new("session").unwrap())
        .unwrap();
    let pending =
        PendingQuestion::open(store.clone(), request(), ActorId::new("host").unwrap()).unwrap();
    let responder = pending.responder();
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute_batch("CREATE TRIGGER fail_answer BEFORE INSERT ON agent_bridge_records WHEN NEW.payload_json LIKE '%\"type\":\"answer\"%' BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    assert!(
        responder
            .answer(ActorId::new("person").unwrap(), selected("review"))
            .is_err()
    );
    assert_eq!(
        store
            .get(&RecordId::new("question").unwrap())
            .unwrap()
            .state,
        RecordState::Open
    );
    assert_eq!(
        store
            .list(&SessionId::new("session").unwrap(), None, 100)
            .unwrap()
            .len(),
        1
    );
    sql.execute_batch("DROP TRIGGER fail_answer").unwrap();
    let answer = responder
        .answer(ActorId::new("person").unwrap(), selected("review"))
        .unwrap();
    assert_eq!(
        *pending.wait(CancellationToken::new()).await.unwrap(),
        *answer
    );
    drop(responder);
    drop(sql);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn form_values_preserve_types_required_presence_and_bounds() {
    let form = Question {
        title: "Details".into(),
        fields: vec![
            QuestionField {
                id: "text".into(),
                label: "Text".into(),
                required: true,
                kind: QuestionFieldKind::Text { max_bytes: 4 },
            },
            QuestionField {
                id: "flag".into(),
                label: "Flag".into(),
                required: true,
                kind: QuestionFieldKind::Boolean,
            },
            QuestionField {
                id: "number".into(),
                label: "Number".into(),
                required: false,
                kind: QuestionFieldKind::Integer { min: 0, max: 5 },
            },
        ],
    };
    let valid = BTreeMap::from([
        ("text".into(), AnswerValue::Text("ok".into())),
        ("flag".into(), AnswerValue::Boolean(false)),
    ]);
    form.validate_answer(&AnswerOutcome::Submitted(valid.clone()))
        .unwrap();
    for (key, value) in [
        ("text", AnswerValue::Text("too long".into())),
        ("text", AnswerValue::Text(" ".into())),
        ("number", AnswerValue::Integer(6)),
        ("extra", AnswerValue::Boolean(true)),
    ] {
        let mut values = valid.clone();
        values.insert(key.into(), value);
        assert!(
            form.validate_answer(&AnswerOutcome::Submitted(values))
                .is_err()
        );
    }
    assert!(form.validate_answer(&AnswerOutcome::Cancelled).is_ok());
    assert!(form.validate_answer(&AnswerOutcome::Declined).is_ok());
    let mut invalid = question();
    invalid.fields.push(invalid.fields[0].clone());
    assert!(invalid.validate().is_err());
}

#[cfg(feature = "questions")]
#[tokio::test]
async fn local_question_waits_for_a_persisted_answer_and_drop_records_cancellation() {
    use agent_bridge::questions::{CancellationToken, PendingQuestion};
    let store: Arc<dyn RecordStore> = Arc::new(MemoryStore::default());
    store
        .create_session(SessionId::new("session").unwrap())
        .unwrap();
    let pending =
        PendingQuestion::open(store.clone(), request(), ActorId::new("host").unwrap()).unwrap();
    let responder = pending.responder();
    assert!(
        responder
            .answer(ActorId::new("person").unwrap(), selected("bad"))
            .is_err()
    );
    let answer = responder
        .answer(ActorId::new("person").unwrap(), selected("draft"))
        .unwrap();
    assert_eq!(
        *pending.wait(CancellationToken::new()).await.unwrap(),
        *answer
    );
    assert!(
        responder
            .answer(ActorId::new("other").unwrap(), selected("review"))
            .is_err()
    );
    let mut next = request();
    next.id = RecordId::new("question-2").unwrap();
    let pending =
        PendingQuestion::open(store.clone(), next, ActorId::new("host").unwrap()).unwrap();
    let responder = pending.responder();
    drop(pending);
    assert_eq!(
        store.get(&responder.question().record.id).unwrap().state,
        RecordState::Complete
    );
    assert!(
        store
            .list(&SessionId::new("session").unwrap(), None, 100)
            .unwrap()
            .iter()
            .any(|record| matches!(
                &record.record.payload,
                Payload::Answer {
                    outcome: AnswerOutcome::Cancelled,
                    delivery: AnswerDelivery::Stored
                }
            ))
    );
}

#[cfg(feature = "questions")]
#[tokio::test]
async fn cancellation_and_concurrent_answers_resolve_only_once() {
    use agent_bridge::questions::{CancellationToken, PendingQuestion};
    let store = Arc::new(MemoryStore::default());
    store
        .create_session(SessionId::new("session").unwrap())
        .unwrap();
    let pending =
        PendingQuestion::open(store.clone(), request(), ActorId::new("host").unwrap()).unwrap();
    let responder = pending.responder();
    let other = responder.clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let ready = barrier.clone();
    let thread = std::thread::spawn(move || {
        ready.wait();
        responder.answer(ActorId::new("person").unwrap(), selected("draft"))
    });
    barrier.wait();
    let second = other.answer(ActorId::new("person").unwrap(), selected("review"));
    let first = thread.join().unwrap();
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let token = CancellationToken::new();
    token.cancel();
    let resolved = pending.wait(token).await.unwrap();
    assert!(matches!(
        resolved.record.payload,
        Payload::Answer {
            outcome: AnswerOutcome::Submitted(_),
            ..
        }
    ));
    let mut next = request();
    next.id = RecordId::new("cancel-before-answer").unwrap();
    let pending = PendingQuestion::open(store, next, ActorId::new("host").unwrap()).unwrap();
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        pending.wait(token).await.unwrap().record.payload,
        Payload::Answer {
            outcome: AnswerOutcome::Cancelled,
            ..
        }
    ));
}
