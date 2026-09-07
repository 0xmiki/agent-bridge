//! Application-managed parent/child execution with explicit selected context.
use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, ContentBlock, ContextMode, ContextTask, RecordActors,
    RecordedRun, SessionUpdate, StopReason,
};
use agent_bridge::context::ContextLimits;
use agent_bridge::execution::{ChildPlan, ChildRequest, DelegationGrant, ExecutionStore};
use agent_bridge::records::{MessageKind, Payload, RecordStore, SqliteStore};
use agent_bridge::{ActorId, ContextManifest, RunId, SessionId, SlotId, ToolGrant, ToolScope};
use std::{
    error::Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn actors(agent: &str) -> RecordActors {
    RecordActors {
        user: ActorId::new("application").unwrap(),
        agent: ActorId::new(agent).unwrap(),
        host: ActorId::new("host").unwrap(),
    }
}
async fn drain<S: RecordStore>(
    run: &mut RecordedRun<'_, '_, '_, S>,
) -> Result<String, Box<dyn Error>> {
    let mut text = String::new();
    let mut complete = false;
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
            AcpEvent::Finished(reason) => complete = reason == StopReason::EndTurn,
            _ => {}
        }
    }
    if !complete {
        return Err("provider did not finish normally".into());
    }
    Ok(text)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let database = args
        .next()
        .ok_or("usage: acp_child <database> <absolute-workspace> <executable> [args...]")?;
    let workspace = args.next().ok_or("missing workspace")?;
    let mut launch = AcpLaunch::new(args.next().ok_or("missing executable")?);
    for argument in args {
        launch = launch.arg(argument);
    }
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let session_id = SessionId::new(format!("family-{unique}"))?;
    let parent_id = RunId::new(format!("parent-{unique}"))?;
    let child_id = RunId::new(format!("child-{unique}"))?;
    let marker = format!("delegated-context-{unique}");
    let store = SqliteStore::open(&database)?;
    let connection = AcpConnection::connect(launch).await?;
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        let mut parent = connection.new_session(session_id.clone(), SlotId::new("parent-slot")?, &workspace, vec![]).await?;
        { let mut run = parent.start_recorded_run(parent_id.clone(), format!("Remember {marker}. Reply only remembered. Do not use tools."), &store, actors("parent-agent"))?; drain(&mut run).await?; }
        let parent_spec = store.get_run(&parent_id)?;
        let selected = store.list(&session_id, None, 100)?.into_iter().filter(|record| record.record.run_id.as_ref() == Some(&parent_id) && matches!(record.record.payload, Payload::Message { kind:MessageKind::User, .. })).map(|record| record.record.id.clone()).collect();
        let context = ContextManifest { records:selected, ..Default::default() };
        let parent_authority = ToolGrant { issuer:actors("parent-agent").host, subject:actors("parent-agent").agent,
            scope:ToolScope { session:session_id.clone(), slot:parent_spec.slot_id }, tools:vec![] };
        let plan = ChildPlan::new(&store, parent_authority, DelegationGrant { issuer:ActorId::new("host")?, subject:ActorId::new("parent-agent")?, parent:parent_id.clone(), context:context.clone(), tools:vec![] },
            ChildRequest { id:child_id.clone(), slot:SlotId::new("child-slot")?, actor:ActorId::new("child-agent")?, context:context.clone(), tools:vec![] })?;
        let mut child = connection.new_session(session_id.clone(), plan.request().slot.clone(), &workspace, vec![]).await?;
        let mut run = child.start_recorded_child_run(&plan, ContextTask { policy:Default::default(), prompt:"What exact phrase was requested in the selected history? Reply only that phrase. Do not use tools.", manifest:&context, resources:&store,
            limits:ContextLimits { max_items:100, max_resource_bytes:65536 }, max_prompt_bytes:65536, mode:ContextMode::AppendToNative }, &store, actors("child-agent"))?;
        if drain(&mut run).await?.trim() != marker { return Err("child did not use the selected context".into()); }
        Ok::<_, Box<dyn Error>>(())
    }).await;
    let shutdown = connection.shutdown().await;
    result??;
    shutdown?;
    drop(store);
    let reopened = SqliteStore::open(database)?;
    let edge = reopened
        .execution_parent(&child_id)?
        .ok_or("missing execution relation")?;
    if edge.parent != parent_id || !edge.child_authority.tools.is_empty() {
        return Err("child lineage or authority was not preserved".into());
    }
    println!(
        "Fresh child used selected context; parent edge and empty application-tool grant survived SQLite reopen."
    );
    Ok(())
}
