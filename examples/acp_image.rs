//! Retain a PNG, send it through ACP, and verify a simple image answer and receipt.
use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, ContentBlock, ContextMode, ContextTask, RecordActors,
    SessionUpdate, StopReason,
};
use agent_bridge::context::{ContextLimits, Resource, ResourceArchive, ResourceStore};
use agent_bridge::records::{Payload, RecordStore, SqliteStore};
use agent_bridge::{ActorId, ContextManifest, ResourceId, ResourceRef, RunId, SessionId, SlotId};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let database = args.next().ok_or("usage: acp_image <database> <absolute-workspace> <png-file> <expected-color> <executable> [args...]")?;
    let workspace = args.next().ok_or("missing workspace")?;
    let image = args.next().ok_or("missing PNG path")?;
    let expected = args.next().ok_or("missing expected color")?;
    let mut launch = AcpLaunch::new(args.next().ok_or("missing executable")?);
    for arg in args {
        launch = launch.arg(arg);
    }
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let session_id = SessionId::new(format!("image-{unique}"))?;
    let reference = ResourceRef {
        id: ResourceId::new(format!("asset-{unique}"))?,
        revision: "v1".into(),
    };
    let store = SqliteStore::open(&database)?;
    store.put(Resource {
        reference: reference.clone(),
        media_type: "image/png".into(),
        bytes: Arc::from(std::fs::read(image)?),
    })?;
    drop(store);
    let store = SqliteStore::open(&database)?;
    let manifest = ContextManifest {
        resources: vec![reference.clone()],
        ..Default::default()
    };
    let connection = AcpConnection::connect(launch).await?;
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let mut session = connection.new_session(session_id.clone(), SlotId::new("image-example")?, workspace, vec![]).await?;
        let mut run = session.start_recorded_context_run(RunId::new(format!("run-{unique}"))?, ContextTask {
            prompt:"What is the predominant color in the attached image? Reply with one lowercase color word. Do not use tools.",
            manifest:&manifest, resources:&store, limits:ContextLimits { max_items:4, max_resource_bytes:4*1024*1024 },
            max_prompt_bytes:6*1024*1024, mode:ContextMode::AppendImagesToNative,
        }, &store, RecordActors { user:ActorId::new("user")?, agent:ActorId::new("assistant")?, host:ActorId::new("example")? })?;
        let mut answer = String::new(); let mut completed = false;
        while let Some(event) = run.next().await? {
            match event {
                AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => if let ContentBlock::Text(text) = chunk.content { answer.push_str(&text.text); },
                AcpEvent::Permission { id, .. } if run.permission_pending(&id) => run.respond(id, None)?,
                AcpEvent::Finished(reason) => completed = reason == StopReason::EndTurn,
                _ => {}
            }
        }
        if !completed || answer.trim() != expected { return Err(format!("image check failed: completed={completed}, answer={answer:?}").into()); }
        println!("Provider identified the expected image color.");
        Ok::<_, Box<dyn Error>>(())
    }).await;
    let shutdown = connection.shutdown().await;
    result??;
    shutdown?;
    drop(store);
    let reopened = SqliteStore::open(database)?;
    let resource = ResourceStore::get(&reopened, &reference)?;
    let digest = format!("{:x}", Sha256::digest(&resource.bytes));
    let receipts: Vec<_> = reopened
        .list(&session_id, None, 100)?
        .into_iter()
        .filter_map(|record| match &record.record.payload {
            Payload::Extension {
                namespace,
                name,
                data,
            } if namespace == "agent_bridge" && name == "input_receipt" => Some(data.clone()),
            _ => None,
        })
        .collect();
    if receipts.len() != 3
        || receipts[0]["version"] != 2
        || receipts[0]["images"][0]["sha256"] != digest
        || receipts[2]["state"] != "response_received"
    {
        return Err("image receipts did not match retained resource evidence".into());
    }
    println!("Image bytes and matching receipt digest survived SQLite reopen.");
    Ok(())
}
