//! Extract typed task data and preserve post-generation validation evidence.
use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, JsonOutputMode, JsonTask, RecordActors,
};
use agent_bridge::records::{Payload, RecordStore, SqliteStore};
use agent_bridge::structured::JsonContract;
use agent_bridge::{ActorId, RunId, SessionId, SlotId};
use serde::Deserialize;
use std::{
    error::Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskList {
    tasks: Vec<String>,
    count: usize,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let database = args
        .next()
        .ok_or("usage: acp_background <database> <absolute-workspace> <executable> [args...]")?;
    let workspace = args.next().ok_or("missing workspace")?;
    let mut launch = AcpLaunch::new(args.next().ok_or("missing executable")?);
    for argument in args {
        launch = launch.arg(argument);
    }
    let contract = JsonContract::<TaskList>::new("task-list", "v1",
        "Return an object with exactly two fields: tasks, an array of task strings, and count, the number of tasks. Do not use tools.", 16 * 1024)?
        .with_validation(|value| {
            if value.tasks.is_empty() || value.count != value.tasks.len() || value.tasks.iter().any(|task| task.trim().is_empty()) {
                Err("tasks must be nonempty and count must equal the number of tasks".into())
            } else { Ok(()) }
        });
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let session_id = SessionId::new(format!("background-{unique}"))?;
    let store = SqliteStore::open(&database)?;
    let connection = AcpConnection::connect(launch).await?;
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let mut session = connection.new_session(session_id.clone(), SlotId::new("task-extractor")?, workspace, vec![]).await?;
        let mut run = session.start_recorded_json_run(RunId::new(format!("extract-{unique}"))?, JsonTask {
            prompt:"Extract the tasks from this note: Write documentation. Add tests. Update the roadmap.",
            contract:&contract, mode:JsonOutputMode::ValidateReturnedText,
        }, &store, RecordActors { user:ActorId::new("caller")?, agent:ActorId::new("extractor")?, host:ActorId::new("background-example")? })?;
        while let Some(event) = run.next().await? {
            if let AcpEvent::Permission { id, .. } = event && run.permission_pending(&id) { run.respond(id, None)?; }
        }
        let value = run.into_result().ok_or("no result was validated")??;
        if value.count != 3 { return Err("extraction did not produce the expected three tasks".into()); }
        println!("Validated {} tasks: {:?}", value.count, value.tasks);
        Ok::<_, Box<dyn Error>>(())
    }).await;
    let shutdown = connection.shutdown().await;
    result??;
    shutdown?;
    drop(store);
    let reopened = SqliteStore::open(database)?;
    let records = reopened.list(&session_id, None, 100)?;
    if !records.iter().any(|record| matches!(&record.record.payload, Payload::Extension { namespace, name, data }
        if namespace == "agent_bridge" && name == "result_validation" && data["validation"]["status"] == "valid" && data["native_enforcement"] == false)) {
        return Err("validation receipt was not retained".into());
    }
    println!("Validation evidence survived SQLite reopen; no native enforcement was claimed.");
    Ok(())
}
