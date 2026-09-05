#![cfg(feature = "sqlite")]
use agent_bridge::context::{Resource, ResourceArchive, ResourceError, ResourceStore};
use agent_bridge::records::SqliteStore;
use agent_bridge::{ResourceId, ResourceRef};
use std::sync::Arc;

fn resource(revision: &str, bytes: &[u8]) -> Resource {
    Resource {
        reference: ResourceRef {
            id: ResourceId::new("asset").unwrap(),
            revision: revision.into(),
        },
        media_type: "image/png".into(),
        bytes: Arc::from(bytes),
    }
}
fn database() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "resources-{}-{}.sqlite3",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[test]
fn resources_reopen_without_duplicate_blobs_or_revision_overwrites() {
    let path = database();
    let store = SqliteStore::open(&path).unwrap();
    let first = resource("v1", b"same bytes");
    store.put(first.clone()).unwrap();
    store.put(first.clone()).unwrap();
    store.put(resource("v2", b"same bytes")).unwrap();
    assert_eq!(
        store.put(resource("v1", b"different")),
        Err(ResourceError::RevisionConflict)
    );
    let mut different_type = first.clone();
    different_type.media_type = "image/jpeg".into();
    assert_eq!(
        store.put(different_type),
        Err(ResourceError::RevisionConflict)
    );
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(*reopened.get(&first.reference).unwrap(), first);
    assert_eq!(
        reopened.get(&resource("missing", b"").reference),
        Err(ResourceError::Missing)
    );
    let sql = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        sql.query_row(
            "SELECT COUNT(*) FROM agent_bridge_resource_blobs",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        sql.query_row(
            "SELECT COUNT(*) FROM agent_bridge_resource_versions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    sql.execute("UPDATE agent_bridge_resource_blobs SET bytes = X'00'", [])
        .unwrap();
    assert!(matches!(
        reopened.get(&first.reference),
        Err(ResourceError::Store(_))
    ));
    drop(sql);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn concurrent_resource_writes_have_one_immutable_winner() {
    let path = database();
    let a = SqliteStore::open(&path).unwrap();
    let b = SqliteStore::open(&path).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let other = barrier.clone();
    let writer = std::thread::spawn(move || {
        other.wait();
        a.put(resource("v1", b"a"))
    });
    barrier.wait();
    let second = b.put(resource("v1", b"b"));
    let first = writer.join().unwrap();
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let failure = if first.is_err() { first } else { second };
    assert_eq!(failure, Err(ResourceError::RevisionConflict));
    drop(b);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn failed_revision_insert_rolls_back_new_blob() {
    let path = database();
    let store = SqliteStore::open(&path).unwrap();
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute_batch("CREATE TRIGGER fail_revision BEFORE INSERT ON agent_bridge_resource_versions BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    assert!(matches!(
        store.put(resource("v1", b"not retained")),
        Err(ResourceError::Store(_))
    ));
    assert_eq!(
        sql.query_row(
            "SELECT COUNT(*) FROM agent_bridge_resource_blobs",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    sql.execute_batch("DROP TRIGGER fail_revision").unwrap();
    store.put(resource("v1", b"retained")).unwrap();
    sql.pragma_update(None, "foreign_keys", false).unwrap(); // Simulate external corruption.
    sql.execute("DELETE FROM agent_bridge_resource_blobs", [])
        .unwrap();
    assert!(matches!(
        store.get(&resource("v1", b"").reference),
        Err(ResourceError::Store(_))
    ));
    drop(sql);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn v3_database_upgrades_resources_without_changing_record_tables() {
    let path = database();
    let sql = rusqlite::Connection::open(&path).unwrap();
    for migration in [
        include_str!("../src/records/sqlite/migrations/0001_records.sql"),
        include_str!("../src/records/sqlite/migrations/0002_continuations.sql"),
        include_str!("../src/records/sqlite/migrations/0003_run_configuration.sql"),
    ] {
        sql.execute_batch(migration).unwrap();
    }
    sql.execute_batch("UPDATE agent_bridge_schema SET version=3; INSERT INTO agent_bridge_sessions(id) VALUES ('preserved');").unwrap();
    drop(sql);
    let store = SqliteStore::open(&path).unwrap();
    store.put(resource("v1", b"retained")).unwrap();
    let sql = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        sql.query_row(
            "SELECT COUNT(*) FROM agent_bridge_sessions WHERE id='preserved'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        sql.query_row("SELECT version FROM agent_bridge_schema", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    drop(sql);
    drop(store);
    std::fs::remove_file(path).unwrap();
}
