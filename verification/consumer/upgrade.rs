use agent_bridge::{
    Content, RecordId, RunId, SessionId,
    records::{ChangeStore, Payload, RecordStore, SqliteStore},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("missing database")?;
    let store = SqliteStore::open(path)?;
    let session = SessionId::new("legacy-session")?;
    let record = store.get(&RecordId::new("legacy-record")?)?;
    assert!(
        matches!(&record.record.payload, Payload::Message { message, .. }
        if message.content == vec![Content::Text("preserved legacy evidence".into())])
    );
    assert_eq!(record.revision, 3);
    let run = store.get_run(&RunId::new("legacy-run")?)?;
    assert_eq!(run.config, Default::default());
    assert!(run.continuation.is_none());
    let page = store.snapshot_page(&session, None, None, 100)?;
    assert_eq!(page.records, vec![record]);
    assert!(store.changes(&page.cursor, 100)?.records.is_empty());
    assert_eq!(
        store.discover_runs(None, 100)?[0].dispatch,
        "missing_evidence"
    );
    println!("Legacy evidence, configuration defaults, and change backfill passed");
    Ok(())
}
