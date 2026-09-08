//! Direct Rust integration, with explicit ownership and persisted-state checks.
//! SQLite calls here are synchronous. This CLI does not inherit the host worker
//! deadlines; a Tokio timeout cannot preempt blocking storage I/O.
use agent_bridge::{
    ActorId, RecordId, RunId, SessionId, SlotId,
    acp::{
        AcpConnection, AcpEvent, AcpLaunch, ContentBlock, RecordActors, SessionUpdate, StopReason,
    },
    records::{ChangeStore, RecordStore, Snapshot, SqliteStore, receipts},
};
use std::{
    collections::BTreeMap,
    error::Error,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn failures(results: impl IntoIterator<Item = Result<(), String>>) -> Result<(), String> {
    let errors: Vec<_> = results.into_iter().filter_map(Result::err).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn history(
    store: &impl RecordStore,
    session: &SessionId,
) -> Result<Vec<Arc<Snapshot>>, Box<dyn Error>> {
    let mut records = vec![];
    let mut after = None;
    loop {
        let page = store.list(session, after, 100)?;
        after = page.last().map(|record| record.record.sequence);
        let full = page.len() == 100;
        records.extend(page);
        if !full {
            return Ok(records);
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let database = PathBuf::from(args.next().ok_or(
        "usage: rust_integration <absolute-database> <absolute-workspace> <executable> [args...]",
    )?);
    let workspace = PathBuf::from(args.next().ok_or("missing workspace")?);
    let executable = args.next().ok_or("missing executable")?;
    let arguments: Vec<_> = args.collect();
    if !database.is_absolute() || !workspace.is_absolute() {
        return Err("database and workspace must be absolute".into());
    }
    let cleanup = executable.contains("codex-acp")
        || arguments.iter().any(|arg| arg.contains("codex-acp"))
        || std::env::var("AGENT_BRIDGE_CODEX_TEST").as_deref() == Ok("1");
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let session_id = SessionId::new(format!("rust-session-{suffix}"))?;
    let slot_id = SlotId::new(format!("rust-slot-{suffix}"))?;
    let run_id = RunId::new(format!("rust-run-{suffix}"))?;
    let store = SqliteStore::open(&database)?;
    store.create_session(session_id.clone())?;
    // Take the starting cursor before generation. The final change scan must
    // include every record, even when an old record was updated during the run.
    let initial = store.snapshot_page(&session_id, None, None, 100)?;
    assert!(initial.records.is_empty());
    let mut launch = AcpLaunch::new(executable);
    for argument in arguments {
        launch = launch.arg(argument);
    }
    let connection = AcpConnection::connect(launch).await?;
    let work: Result<(), String> = async {
        let mut session = connection
            .new_session(session_id.clone(), slot_id, workspace, vec![])
            .await
            .map_err(|error| error.to_string())?;
        let generated = tokio::time::timeout(Duration::from_secs(60), async {
            let mut run = session
                .start_recorded_run(
                    run_id,
                    "Say hello without using tools.",
                    &store,
                    RecordActors {
                        user: ActorId::new("user").unwrap(),
                        agent: ActorId::new("assistant").unwrap(),
                        host: ActorId::new("rust-example").unwrap(),
                    },
                )
                .map_err(|error| error.to_string())?;
            let mut reason = None;
            while let Some(event) = run.next().await.map_err(|error| error.to_string())? {
                match event {
                    AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            print!("{}", text.text);
                        }
                    }
                    AcpEvent::Permission { id, .. } if run.permission_pending(&id) => {
                        run.respond(id, None).map_err(|error| error.to_string())?
                    }
                    AcpEvent::Finished(value) => {
                        println!("\nStopped: {value:?}");
                        reason = Some(value);
                    }
                    _ => {}
                }
            }
            if reason != Some(StopReason::EndTurn) {
                return Err(format!("run did not complete normally: {reason:?}"));
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|_| Err("generation deadline elapsed; outcome is uncertain".into()));
        // The generation future (and its run) is dropped before consuming the
        // session. Cleanup is attempted even if generation failed or timed out.
        let cleaned = if cleanup {
            session
                .delete()
                .await
                .map_err(|error| format!("provider session cleanup failed: {error}"))
        } else {
            drop(session);
            Ok(())
        };
        failures([generated, cleaned])
    }
    .await;
    let stopped = connection
        .shutdown()
        .await
        .map_err(|error| format!("provider shutdown failed: {error}"));
    failures([work, stopped])?;

    let mut cursor = initial.cursor;
    let mut projection: BTreeMap<RecordId, Arc<Snapshot>> = BTreeMap::new();
    loop {
        let page = store.changes(&cursor, 100)?;
        for record in page.records {
            if projection
                .get(&record.record.id)
                .is_none_or(|old| old.revision < record.revision)
            {
                projection.insert(record.record.id.clone(), record);
            }
        }
        cursor = page.cursor;
        if !page.page_full {
            break;
        }
    }
    let mut projected: Vec<_> = projection.into_values().collect();
    projected.sort_by_key(|record| record.record.sequence);
    let saved = history(&store, &session_id)?;
    assert_eq!(projected, saved);
    let mut receipt_count = 0;
    for record in &saved {
        if receipts::read(&record.record.payload)?.is_some() {
            receipt_count += 1;
        }
    }
    drop(store);
    // Reopening history does not launch another agent or resume native context.
    let reopened = SqliteStore::open_read_only(&database, Duration::from_millis(100))?;
    assert_eq!(history(&reopened, &session_id)?, saved);
    assert!(reopened.changes(&cursor, 100)?.records.is_empty());
    println!(
        "Verified {} records after reopening; read {receipt_count} receipt views.",
        saved.len()
    );
    Ok(())
}
