//! The same binary hosts an application tool or runs an agent using that tool.
use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, ContentBlock, McpServer, McpServerStdio, RecordActors,
    SessionUpdate, StopReason,
};
use agent_bridge::mcp::McpToolServer;
use agent_bridge::records::{MemoryStore, Payload, RecordStore};
use agent_bridge::tools::{ToolError, ToolGrant, ToolRef, ToolRegistry, ToolScope};
use agent_bridge::{ActorId, RunId, SessionId, SlotId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::{
    error::Error,
    io::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Lookup {
    key: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let first = args
        .next()
        .ok_or("usage: application_tools <absolute-workspace> <agent-executable> [args...]")?;
    if first == "serve" {
        let ledger = PathBuf::from(args.next().ok_or("missing ledger")?);
        let session = SessionId::new(args.next().ok_or("missing application session")?)?;
        let host = ActorId::new("example")?;
        let actor = ActorId::new("assistant")?;
        let scope = ToolScope {
            session,
            slot: SlotId::new("application-slot")?,
        };
        let reference = ToolRef {
            name: "application_lookup".into(),
            revision: "v1".into(),
        };
        let mut registry = ToolRegistry::default();
        registry.register::<Lookup, _, _, _>(reference.clone(), "Look up application project data with key 'project'. Returns a verification token.", move |context, input| {
            let ledger = ledger.clone();
            async move {
                if input.key != "project" { return Err(ToolError::InvalidArguments("key must be project".into())); }
                let token = format!("application-proof-{}", SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| ToolError::Handler(e.to_string()))?.as_nanos());
                let value = json!({"project":"agent-bridge","verification_token":token,"session":context.scope.session.as_str()});
                let mut file = std::fs::OpenOptions::new().create(true).append(true).open(ledger).map_err(|e| ToolError::Handler(e.to_string()))?;
                writeln!(file, "{value}").map_err(|e| ToolError::Handler(e.to_string()))?;
                Ok(value)
            }
        })?;
        let grant = ToolGrant {
            issuer: host.clone(),
            subject: actor.clone(),
            scope: scope.clone(),
            tools: vec![reference],
        };
        McpToolServer::new(Arc::new(registry), grant, scope, actor, host)?
            .serve_stdio()
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let workspace = first;
    let mut launch = AcpLaunch::new(args.next().ok_or("missing agent executable")?);
    for arg in args {
        launch = launch.arg(arg);
    }
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = std::env::temp_dir().join(format!("bridge-application-tools-{unique}"));
    std::fs::create_dir(&directory)?;
    let ledger = directory.join("calls.jsonl");
    let session_id = SessionId::new(format!("application-{unique}"))?;
    let server = McpServer::Stdio(
        McpServerStdio::new("application", std::env::current_exe()?).args(vec![
            "serve".into(),
            ledger.to_string_lossy().into_owned(),
            session_id.to_string(),
        ]),
    );
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch).await?;
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        let mut session = connection.new_session(session_id.clone(), SlotId::new("application-slot")?, workspace, vec![server]).await?;
        let mut run = session.start_recorded_run(RunId::new(format!("lookup-{unique}"))?, "Call the application_lookup MCP tool with key project. Reply only with its verification_token. Do not use other tools.", &store,
            RecordActors { user:ActorId::new("caller")?, agent:ActorId::new("assistant")?, host:ActorId::new("example")? })?;
        let mut text = String::new(); let mut complete = false;
        while let Some(event) = run.next().await? {
            match event {
                AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => if let ContentBlock::Text(chunk) = chunk.content { text.push_str(&chunk.text); },
                AcpEvent::Permission { id, .. } if run.permission_pending(&id) => run.respond(id, None)?,
                AcpEvent::Finished(reason) => complete = reason == StopReason::EndTurn,
                _ => {}
            }
        }
        let calls = std::fs::read_to_string(&ledger)?;
        let mut matched = false;
        for line in calls.lines() {
            let call: serde_json::Value = serde_json::from_str(line)?;
            matched |= call["verification_token"].as_str() == Some(text.trim()) && call["session"] == session_id.as_str();
        }
        if !complete || !matched { return Err("provider did not return a scoped application tool result".into()); }
        Ok::<_, Box<dyn Error>>(())
    }).await;
    let shutdown = connection.shutdown().await;
    result??;
    shutdown?;
    if !store
        .list(&session_id, None, 100)?
        .iter()
        .any(|r| matches!(r.record.payload, Payload::Tool(_)))
    {
        return Err("no portable tool activity recorded".into());
    }
    println!(
        "Typed application tool executed in its granted scope; returned value and recorded tool activity verified."
    );
    println!("Evidence: {}", ledger.display());
    Ok(())
}
