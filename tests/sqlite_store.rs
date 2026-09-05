#![cfg(feature = "sqlite")]
use agent_bridge::records::*;
use agent_bridge::{
    ActorId, Content, ContextManifest, InstructionRef, InstructionRole, Message, RecordId,
    ResourceId, ResourceRef, RunId, RunSpec, SessionId, SlotId,
};
use rusqlite::{Connection, params};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
};

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "sqlite-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("records.sqlite3")
    }
    fn open(&self) -> SqliteStore {
        SqliteStore::open(self.path()).unwrap()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn spec(id: &str) -> RunSpec {
    RunSpec {
        id: RunId::new(id).unwrap(),
        session_id: SessionId::new("s").unwrap(),
        slot_id: SlotId::new("slot").unwrap(),
        context: ContextManifest::default(),
        config: (),
    }
}
fn ready(store: &SqliteStore) {
    store.create_session(SessionId::new("s").unwrap()).unwrap();
    store.register_run(spec("r")).unwrap();
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
        source: Some(SourceRef {
            namespace: "acp".into(),
            id: "native/message".into(),
        }),
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
fn decision(id: &str, option: &str) -> Draft {
    let mut draft = draft(
        id,
        Payload::Decision {
            outcome: PermissionOutcome::Selected(option.into()),
            delivery: DecisionDelivery::Queued,
        },
        RecordState::Complete,
    );
    draft.reply_to_id = Some(RecordId::new("permission").unwrap());
    draft
}

#[test]
fn reopens_all_payloads_context_and_idempotency_state() {
    let database = Database::new();
    let store = database.open();
    ready(&store);
    let original = draft("message", message(""), RecordState::Open);
    let first = store.insert(original.clone()).unwrap();
    store
        .checkpoint(&first.record.id, 0, message("part"), RecordState::Open)
        .unwrap();
    let final_message = store
        .checkpoint(
            &first.record.id,
            1,
            message("Hello 'quoted' 🌍\nnext line"),
            RecordState::Complete,
        )
        .unwrap();
    let resource = ResourceRef {
        id: ResourceId::new("image").unwrap(),
        revision: "rev1".into(),
    };
    let payloads = vec![
        Payload::Message {
            kind: MessageKind::User,
            message: Message {
                content: vec![Content::Resource(resource.clone())],
            },
        },
        Payload::Message {
            kind: MessageKind::Reasoning,
            message: Message { content: vec![] },
        },
        Payload::Tool(ToolActivity {
            title: "Read".into(),
            status: ToolStatus::Completed,
            input: Some(json!({"count":u64::MAX})),
            output: Some(json!([true, null])),
            extensions: BTreeMap::from([("custom.tool".into(), json!({"nested":{"a":[1,2]}}))]),
        }),
        Payload::Failure {
            message: "interrupted".into(),
        },
        Payload::RunFinished {
            reason: CompletionReason::TokenLimit,
        },
        Payload::Extension {
            namespace: "future.app".into(),
            name: "widget".into(),
            data: json!({"shape":["a", 12]}),
        },
    ];
    for (index, payload) in payloads.into_iter().enumerate() {
        store
            .insert(draft(
                &format!("record-{index}"),
                payload,
                RecordState::Complete,
            ))
            .unwrap();
    }
    let request = store.insert(permission()).unwrap();
    let response_draft = decision("response", "allow");
    let response = store
        .resolve(&request.record.id, 0, response_draft.clone())
        .unwrap();
    let mut context_run = spec("context-run");
    context_run.context = ContextManifest {
        records: vec![first.record.id.clone()],
        resources: vec![resource.clone()],
        instructions: vec![InstructionRef {
            resource,
            role: InstructionRole::Base,
        }],
    };
    store.register_run(context_run.clone()).unwrap();
    let history = store
        .list(&SessionId::new("s").unwrap(), None, 100)
        .unwrap();
    drop(store);

    let reopened = database.open();
    assert_eq!(
        reopened
            .list(&SessionId::new("s").unwrap(), None, 100)
            .unwrap(),
        history
    );
    assert_eq!(reopened.get_run(&context_run.id).unwrap(), context_run);
    assert_eq!(reopened.insert(original).unwrap(), final_message);
    assert_eq!(
        reopened
            .resolve(&request.record.id, 0, response_draft)
            .unwrap(),
        response
    );
    let next = reopened
        .insert(draft("next", message("next"), RecordState::Complete))
        .unwrap();
    assert_eq!(next.record.sequence, history.len() as u64);
}

#[test]
fn migrations_coexist_with_application_tables_and_user_version() {
    let database = Database::new();
    let connection = Connection::open(database.path()).unwrap();
    connection.execute_batch("CREATE TABLE application_data(value TEXT); INSERT INTO application_data VALUES ('keep'); PRAGMA user_version = 42;").unwrap();
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    drop(connection);
    drop(database.open());
    drop(database.open());
    let connection = Connection::open(database.path()).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        42
    );
    assert_eq!(
        connection
            .query_row("SELECT value FROM application_data", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    assert_eq!(
        connection
            .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        journal
    );
    assert_eq!(
        connection
            .query_row("SELECT version FROM agent_bridge_schema", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn newer_schemas_and_unversioned_reserved_tables_are_rejected() {
    let database = Database::new();
    drop(database.open());
    let connection = Connection::open(database.path()).unwrap();
    connection
        .execute("UPDATE agent_bridge_schema SET version = 99", [])
        .unwrap();
    assert!(matches!(
        SqliteStore::open(database.path()),
        Err(StoreError::UnsupportedSchemaVersion(99))
    ));
    assert_eq!(
        connection
            .query_row("SELECT version FROM agent_bridge_schema", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        99
    );
    let other = Database::new();
    let connection = Connection::open(other.path()).unwrap();
    connection.execute_batch("CREATE TABLE agent_bridge_records(value TEXT); INSERT INTO agent_bridge_records VALUES ('keep');").unwrap();
    assert!(matches!(
        SqliteStore::open(other.path()),
        Err(StoreError::UnversionedSchema)
    ));
    assert_eq!(
        connection
            .query_row("SELECT value FROM agent_bridge_records", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
}

#[test]
fn json_v1_shape_is_explicit_and_corruption_is_not_silently_skipped() {
    let database = Database::new();
    let store = database.open();
    ready(&store);
    store
        .insert(draft("m", message("hello"), RecordState::Complete))
        .unwrap();
    let connection = Connection::open(database.path()).unwrap();
    let encoded: String = connection
        .query_row(
            "SELECT payload_json FROM agent_bridge_records WHERE id = 'm'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
        json!({
            "version":1,"data":{"type":"message","data":{"kind":"agent","message":{"content":[{"type":"text","data":"hello"}]}}}
        })
    );
    let mut newer: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    newer["version"] = json!(2);
    connection
        .execute(
            "UPDATE agent_bridge_records SET payload_json = ?1 WHERE id = 'm'",
            [newer.to_string()],
        )
        .unwrap();
    assert!(matches!(
        store.get(&RecordId::new("m").unwrap()),
        Err(StoreError::UnsupportedDataVersion(2))
    ));
    assert!(matches!(
        store.list(&SessionId::new("s").unwrap(), None, 100),
        Err(StoreError::UnsupportedDataVersion(2))
    ));
    connection
        .execute(
            "UPDATE agent_bridge_records SET payload_json = '{broken' WHERE id = 'm'",
            [],
        )
        .unwrap();
    assert!(matches!(
        store.get(&RecordId::new("m").unwrap()),
        Err(StoreError::CorruptData(_))
    ));
    let after: String = connection
        .query_row(
            "SELECT payload_json FROM agent_bridge_records WHERE id = 'm'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, "{broken");
}

#[test]
fn serialized_context_ids_still_use_identifier_validation() {
    let database = Database::new();
    let store = database.open();
    ready(&store);
    let connection = Connection::open(database.path()).unwrap();
    connection
        .execute(
            "UPDATE agent_bridge_runs SET context_json = ?1 WHERE id = 'r'",
            [json!({
                "version":1,"data":{"records":["   "],"instructions":[],"resources":[]}
            })
            .to_string()],
        )
        .unwrap();
    assert!(matches!(
        store.get_run(&RunId::new("r").unwrap()),
        Err(StoreError::CorruptData(_))
    ));
}

#[test]
fn late_sql_failure_rolls_back_request_response_and_sequence() {
    let database = Database::new();
    let store = database.open();
    ready(&store);
    let request = store.insert(permission()).unwrap();
    let connection = Connection::open(database.path()).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_decision BEFORE INSERT ON agent_bridge_decisions BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    assert!(matches!(
        store.resolve(&request.record.id, 0, decision("response", "allow")),
        Err(StoreError::Database(_))
    ));
    assert_eq!(store.get(&request.record.id).unwrap(), request);
    assert!(matches!(
        store.get(&RecordId::new("response").unwrap()),
        Err(StoreError::MissingRecord)
    ));
    assert_eq!(
        store
            .insert(draft("next", message("next"), RecordState::Complete))
            .unwrap()
            .record
            .sequence,
        1
    );
    connection
        .execute_batch("DROP TRIGGER reject_decision;")
        .unwrap();
    store
        .resolve(&request.record.id, 0, decision("response", "allow"))
        .unwrap();
}

#[test]
fn independent_connections_serialize_decisions_and_checkpoints() {
    let database = Database::new();
    let store = database.open();
    ready(&store);
    let request = store.insert(permission()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = ["allow", "reject"]
        .into_iter()
        .map(|option| {
            let store = database.open();
            let barrier = barrier.clone();
            let id = request.record.id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.resolve(&id, 0, decision(option, option))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::AlreadyResolved)))
            .count(),
        1
    );

    let record = store
        .insert(draft("m", message(""), RecordState::Open))
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|text| {
            let store = database.open();
            let barrier = barrier.clone();
            let id = record.record.id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.checkpoint(&id, 0, message(text), RecordState::Complete)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::RevisionConflict)))
            .count(),
        1
    );
}

#[test]
fn independent_connections_allocate_sequences_without_gaps() {
    let database = Database::new();
    let store = database.open();
    ready(&store);
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|index| {
            let store = database.open();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.insert(draft(
                    &index.to_string(),
                    message("x"),
                    RecordState::Complete,
                ))
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap().unwrap();
    }
    let sequences: Vec<_> = store
        .list(&SessionId::new("s").unwrap(), None, 100)
        .unwrap()
        .iter()
        .map(|r| r.record.sequence)
        .collect();
    assert_eq!(sequences, (0..8).collect::<Vec<_>>());
}

#[test]
fn sequence_exhaustion_does_not_partially_insert_a_record() {
    let database = Database::new();
    let store = database.open();
    ready(&store);
    let connection = Connection::open(database.path()).unwrap();
    connection
        .execute(
            "UPDATE agent_bridge_sessions SET next_sequence = ?1 WHERE id = 's'",
            params![i64::MAX],
        )
        .unwrap();
    assert!(matches!(
        store.insert(draft("m", message("x"), RecordState::Complete)),
        Err(StoreError::SequenceExhausted)
    ));
    assert!(matches!(
        store.get(&RecordId::new("m").unwrap()),
        Err(StoreError::MissingRecord)
    ));
}

#[test]
fn open_records_are_not_automatically_replayed_or_finalized_on_reopen() {
    let database = Database::new();
    let store = database.open();
    ready(&store);
    let record = store
        .insert(draft("partial", message("partial"), RecordState::Open))
        .unwrap();
    drop(store);
    let reopened = database.open();
    assert_eq!(reopened.get(&record.record.id).unwrap(), record);
    assert_eq!(reopened.register_run(spec("r")), Ok(false));
}
