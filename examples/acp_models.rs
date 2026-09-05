//! List available models or run two selected models on one native conversation.
use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, AcpSession, ContentBlock, RecordActors, SessionUpdate,
};
use agent_bridge::records::{RecordStore, SqliteStore};
use agent_bridge::{ActorId, ConfigValue, RunId, SessionId, SlotId};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

async fn prompt(
    session: &mut AcpSession<'_>,
    store: &SqliteStore,
    id: RunId,
    input: String,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut run = session.start_recorded_run(
        id,
        input,
        store,
        RecordActors {
            user: ActorId::new("user")?,
            agent: ActorId::new("assistant")?,
            host: ActorId::new("example")?,
        },
    )?;
    println!("Run configuration: {:?}", run.run().spec().config);
    let mut output = String::new();
    while let Some(event) = run.next().await? {
        match event {
            AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                if let ContentBlock::Text(text) = chunk.content {
                    output.push_str(&text.text);
                }
            }
            AcpEvent::Permission { id, .. } if run.permission_pending(&id) => {
                run.respond(id, None)?
            }
            _ => {}
        }
    }
    println!("Answer: {}", output.trim());
    Ok(output)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let database = PathBuf::from(args.next().ok_or(
        "usage: acp_models <db> <workspace> <model-a|list> <model-b|-> <executable> [args...]",
    )?);
    let workspace = args.next().ok_or("missing workspace")?;
    let model_a = args.next().ok_or("missing first model or list")?;
    let model_b = args.next().ok_or("missing second model or -")?;
    let executable = args.next().ok_or("missing executable")?;
    let mut launch = AcpLaunch::new(executable);
    for arg in args {
        launch = launch.arg(arg);
    }
    let connection = AcpConnection::connect(launch).await?;
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let app_id = SessionId::new(format!("models-{unique}"))?;
        let mut session = connection.new_session(app_id, SlotId::new("model-example")?, workspace, vec![]).await?;
        let configuration = session.configuration();
        if model_a == "list" {
            println!("Current: {:?}", configuration.values.confirmed);
            for option in configuration.options.unwrap_or_default() {
                if option.category.as_deref() == Some("model") {
                    for choice in option.choices { println!("{}: {}", choice.value, choice.label); }
                }
            }
            return Ok::<_, Box<dyn std::error::Error>>(());
        }
        if model_a == model_b { return Err("choose two different models".into()); }
        let store = SqliteStore::open(&database)?;
        let native_id = session.info().session_id.clone();
        let first_id = RunId::new(format!("{unique}-a"))?;
        let second_id = RunId::new(format!("{unique}-b"))?;
        let marker = format!("bridge-model-memory-{unique}");
        session.set_model(model_a.clone()).await?;
        prompt(&mut session, &store, first_id.clone(), format!("Remember the exact phrase {marker}. Reply only remembered. Do not use tools.")).await?;
        session.set_model(model_b.clone()).await?;
        let answer = prompt(&mut session, &store, second_id.clone(), "What exact phrase did I ask you to remember? Reply only that phrase. Do not use tools.".into()).await?;
        assert_eq!(native_id, session.info().session_id);
        drop(store);
        let reopened = SqliteStore::open(&database)?;
        let a = reopened.get_run(&first_id)?;
        let b = reopened.get_run(&second_id)?;
        assert_eq!(a.session_id, b.session_id);
        assert!(a.config.confirmed.as_ref().is_some_and(|values| values.values().any(|v| v == &ConfigValue::Select(model_a.clone()))));
        assert!(b.config.confirmed.as_ref().is_some_and(|values| values.values().any(|v| v == &ConfigValue::Select(model_b.clone()))));
        if answer.trim() != marker { return Err("second model did not recall the expected phrase".into()); }
        println!("Two models shared native context; both run configurations survived SQLite reopen.");
        Ok(())
    }).await;
    let shutdown = connection.shutdown().await;
    result??;
    shutdown?;
    Ok(())
}
