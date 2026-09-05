//! Persist and reopen a user record. No agent is launched by this example.
use agent_bridge::records::{Draft, MessageKind, Payload, RecordState, RecordStore, SqliteStore};
use agent_bridge::{ActorId, Content, Message, RecordId, SessionId};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: sqlite_history <database-path>")?;
    let session = SessionId::new("example-history")?;
    let store = SqliteStore::open(&path)?;
    store.create_session(session.clone())?;
    store.insert(Draft {
        id: RecordId::new("example-input")?,
        session_id: session.clone(),
        run_id: None,
        actor: ActorId::new("user")?,
        reply_to_id: None,
        source: None,
        payload: Payload::Message {
            kind: MessageKind::User,
            message: Message {
                content: vec![Content::Text("Hello from SQLite.".into())],
            },
        },
        state: RecordState::Complete,
    })?;
    drop(store);

    let reopened = SqliteStore::open(&path)?;
    let records = reopened.list(&session, None, 100)?;
    for snapshot in &records {
        println!(
            "{}: {:?}",
            snapshot.record.sequence, snapshot.record.payload
        );
    }
    println!(
        "{} record(s) read after reopening. Repeating this example does not duplicate the input.",
        records.len()
    );
    Ok(())
}
