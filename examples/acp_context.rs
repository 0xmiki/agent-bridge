//! Supply selected history and supplemental instructions, then reopen input receipts.
use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, ContentBlock, ContextTask, RecordActors, SessionUpdate,
    TextContextMode,
};
use agent_bridge::context::{ContextLimits, MemoryResourceStore, Resource};
use agent_bridge::records::{Draft, MessageKind, Payload, RecordState, RecordStore, SqliteStore};
use agent_bridge::{
    ActorId, Content, ContextManifest, InstructionRef, InstructionRole, Message, RecordId,
    ResourceId, ResourceRef, RunId, SessionId, SlotId,
};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let database = args
        .next()
        .ok_or("usage: acp_context <database> <absolute-workspace> <executable> [args...]")?;
    let workspace = args.next().ok_or("missing workspace")?;
    let mut launch = AcpLaunch::new(args.next().ok_or("missing executable")?);
    for argument in args {
        launch = launch.arg(argument);
    }
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let session_id = SessionId::new(format!("context-{unique}"))?;
    let marker = format!("shipping-{unique}");
    let store = SqliteStore::open(&database)?;
    store.create_session(session_id.clone())?;
    let selected = store.insert(Draft {
        id: RecordId::new(format!("history-{unique}"))?,
        session_id: session_id.clone(),
        run_id: None,
        actor: ActorId::new("participant")?,
        reply_to_id: None,
        source: None,
        state: RecordState::Complete,
        payload: Payload::Message {
            kind: MessageKind::User,
            message: Message {
                content: vec![Content::Text(format!("The shipping codename is {marker}."))],
            },
        },
    })?;
    let resources = MemoryResourceStore::default();
    let instruction = ResourceRef {
        id: ResourceId::new("answer-style")?,
        revision: "v1".into(),
    };
    resources.put(Resource { reference: instruction.clone(), media_type: "text/plain".into(), bytes: Arc::from(b"Answer the task using the selected history. Reply only with the exact codename. Do not use tools.".as_slice()) })?;
    resources.put(Resource { reference: ResourceRef { id:instruction.id.clone(), revision:"v2".into() }, media_type:"text/plain".into(), bytes:Arc::from(b"For this turn, supersede the earlier answer style. Reply only with the shipping codename in UPPERCASE. Do not use tools.".as_slice()) })?;
    let manifest = ContextManifest {
        records: vec![selected.record.id.clone()],
        instructions: vec![InstructionRef {
            resource: instruction,
            role: InstructionRole::Supplemental,
        }],
        resources: vec![],
    };
    let connection = AcpConnection::connect(launch).await?;
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let mut session = connection
            .new_session(
                session_id.clone(),
                SlotId::new("context-example")?,
                workspace,
                vec![],
            )
            .await?;
        for index in 0..2 {
            let mut current = manifest.clone();
            if index == 1 {
                current.instructions[0].resource.revision = "v2".into();
            }
            let mut policy = agent_bridge::context::ContextPolicy::for_host(
                ActorId::new("example")?,
                current.instructions.clone(),
            );
            if index == 1 {
                let optional = ResourceRef {
                    id: ResourceId::new("optional-context")?,
                    revision: "v1".into(),
                };
                current.resources.push(optional.clone());
                policy
                    .omissions
                    .push(agent_bridge::context::ContextOmission {
                        item: agent_bridge::context::ContextItem::Resource(optional),
                        reason: "not needed for this turn".into(),
                    });
            }
            let mut run = session.start_recorded_context_run(
                RunId::new(format!("run-{unique}-{index}"))?,
                ContextTask {
                    policy,
                    prompt: "What is the shipping codename?",
                    manifest: &current,
                    resources: &resources,
                    limits: ContextLimits {
                        max_items: 10,
                        max_resource_bytes: 4096,
                    },
                    max_prompt_bytes: 16384,
                    mode: TextContextMode::AppendToNative,
                },
                &store,
                RecordActors {
                    user: ActorId::new("user")?,
                    agent: ActorId::new("assistant")?,
                    host: ActorId::new("example")?,
                },
            )?;
            let mut answer = String::new();
            while let Some(event) = run.next().await? {
                match event {
                    AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            answer.push_str(&text.text);
                        }
                    }
                    AcpEvent::Permission { id, .. } if run.permission_pending(&id) => {
                        run.respond(id, None)?
                    }
                    _ => {}
                }
            }
            let expected = if index == 0 {
                marker.clone()
            } else {
                marker.to_uppercase()
            };
            if answer.trim() != expected {
                return Err("agent did not return the selected history's codename".into());
            }
            println!("Selected history and supplemental text produced the expected answer.");
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await;
    let shutdown = connection.shutdown().await;
    result??;
    shutdown?;
    drop(store);
    drop(resources);
    let reopened = SqliteStore::open(database)?;
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
    if receipts.len() != 6
        || receipts[0]["state"] != "prepared"
        || receipts[1]["state"] != "dispatch_attempted"
        || receipts[2]["state"] != "response_received"
        || receipts[3]["instruction_authority"]["granted_instructions"][0]["resource"]["revision"]
            != "v2"
        || receipts[0]["instruction_authority"]["granted_instructions"][0]["resource"]["revision"]
            != "v1"
        || receipts[3]["omissions"].as_array().map(Vec::len) != Some(1)
    {
        return Err("input receipt sequence was not preserved".into());
    }
    println!(
        "Instruction revisions, omission reasons, and delivery evidence survived SQLite reopen."
    );
    Ok(())
}
