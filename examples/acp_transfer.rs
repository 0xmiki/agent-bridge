//! Transfer selected recorded conversation between two provider processes.
use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, ContentBlock, ContextMode, PortableRestore, RecordActors,
    RecordedRun, RestorationPolicy, SessionUpdate, StopReason,
};
use agent_bridge::context::ContextLimits;
use agent_bridge::records::{MessageKind, Payload, RecordStore, SqliteStore};
use agent_bridge::{ActorId, ContextManifest, RunId, SessionId, SlotId};
use std::{
    error::Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn actors() -> RecordActors {
    RecordActors {
        user: ActorId::new("person").unwrap(),
        agent: ActorId::new("assistant").unwrap(),
        host: ActorId::new("transfer-example").unwrap(),
    }
}
fn launch(arguments: &[String]) -> Result<AcpLaunch, Box<dyn Error>> {
    let mut launch = AcpLaunch::new(arguments.first().ok_or("missing provider executable")?);
    for argument in &arguments[1..] {
        launch = launch.arg(argument);
    }
    Ok(launch)
}
async fn drain<S: RecordStore>(
    run: &mut RecordedRun<'_, '_, '_, S>,
) -> Result<String, Box<dyn Error>> {
    let mut text = String::new();
    let mut completed = false;
    while let Some(event) = run.next().await? {
        match event {
            AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                if let ContentBlock::Text(chunk) = chunk.content {
                    text.push_str(&chunk.text);
                }
            }
            AcpEvent::Permission { id, .. } if run.permission_pending(&id) => {
                run.respond(id, None)?
            }
            AcpEvent::Finished(reason) => completed = reason == StopReason::EndTurn,
            _ => {}
        }
    }
    if !completed {
        return Err("provider did not finish normally".into());
    }
    Ok(text)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let database = args.first().ok_or(
        "usage: acp_transfer <db> <workspace> <source> [args...] --to <destination> [args...]",
    )?;
    let workspace = args.get(1).ok_or("missing workspace")?;
    let split = args
        .iter()
        .position(|arg| arg == "--to")
        .ok_or("missing --to")?;
    if split < 3 {
        return Err("missing source executable".into());
    }
    let source_launch = launch(&args[2..split])?;
    let destination_launch = launch(&args[split + 1..])?;
    tokio::time::timeout(Duration::from_secs(120), async {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let session_id = SessionId::new(format!("transfer-{unique}"))?;
        let marker = format!("transfer-memory-{unique}");
        let store = SqliteStore::open(database)?;
        let source = AcpConnection::connect(source_launch).await?;
        println!("Source: {:?}", source.info().agent_info);
        let source_result = async {
            let mut session = source.new_session(session_id.clone(), SlotId::new("source-slot")?, workspace, vec![]).await?;
            let mut run = session.start_recorded_run(RunId::new(format!("first-{unique}"))?, format!("Remember {marker}. Reply only remembered. Do not use tools."), &store, actors())?;
            drain(&mut run).await?;
            Ok::<_, Box<dyn Error>>(())
        }.await;
        let shutdown = source.shutdown().await; source_result?; shutdown?; drop(store);
        let store = SqliteStore::open(database)?;
        let selected = store.list(&session_id, None, 100)?.into_iter().filter(|r| matches!(r.record.payload, Payload::Message { kind: MessageKind::User | MessageKind::Agent, .. })).map(|r| r.record.id.clone()).collect();
        let destination = AcpConnection::connect(destination_launch).await?;
        println!("Destination: {:?}", destination.info().agent_info);
        let result = async {
            let mut restored = destination.restore(RestorationPolicy::Portable(PortableRestore {
                session_id:session_id.clone(), slot_id:SlotId::new("destination-slot")?, cwd:workspace.into(),
                manifest:ContextManifest { records:selected, ..Default::default() }, limits:ContextLimits { max_items:100, max_resource_bytes:65536 },
                max_prompt_bytes:65536, mode:ContextMode::AppendToNative,
            }), &store, &store, vec![]).await?;
            println!("Restoration report: {}", restored.report());
            let mut run = restored.start_recorded_run(RunId::new(format!("second-{unique}"))?, "What exact phrase did I ask you to remember? Reply only that phrase. Do not use tools.", &store, actors())?;
            let answer = drain(&mut run).await?;
            if answer.trim() != marker { return Err("destination did not recall selected history".into()); }
            Ok::<_, Box<dyn Error>>(())
        }.await;
        let shutdown = destination.shutdown().await; result?; shutdown?; drop(store);
        let reopened = SqliteStore::open(database)?;
        let records = reopened.list(&session_id, None, 100)?;
        if !records.iter().any(|r| matches!(&r.record.payload, Payload::Extension { namespace, name, data } if namespace == "agent_bridge" && name == "restoration" && data["strategy"] == "portable_selection")) {
            return Err("portable restoration report was not retained".into());
        }
        println!("Destination recalled selected history in the same application session; restoration evidence survived reopen.");
        Ok::<_, Box<dyn Error>>(())
    }).await??;
    Ok(())
}
