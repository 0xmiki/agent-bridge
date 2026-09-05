//! Opt-in real-provider checks. Use a disposable workspace and configured login.
use agent_bridge::acp::{
    AcpEvent, AcpSession, ContentBlock, McpServer, McpServerStdio, SessionUpdate, StopReason,
};
use agent_bridge::providers::{AcpDriver, ExecutableSearch, ProviderDefinition, ProviderDriver};
use agent_bridge::records::SqliteStore;
use agent_bridge::{ConfigValue, ContinuationId, RunId, RunStatus, SessionId, SlotId};
use agent_client_protocol::schema::v1::PermissionOptionKind;
use serde_json::{Value, json};
use std::{error::Error, path::Path, time::Duration};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn ledger(path: &Path) -> Result<Vec<Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(error) => Err(error.into()),
    }
}

#[derive(Default)]
struct Observation {
    text: String,
    permissions: usize,
    fixture_permissions: usize,
    allowed: usize,
    tool_updates: usize,
    cancelled: bool,
}

async fn prompt(
    session: &mut AcpSession<'_>,
    id: &str,
    input: &str,
    cancel_ledger: Option<&Path>,
    deny: bool,
) -> Result<Observation> {
    tokio::time::timeout(Duration::from_secs(90), async {
        let mut run = session.start_run(RunId::new(id)?, input)?;
        let mut observed = Observation::default();
        let mut tool_titles = std::collections::HashMap::new();
        let mut timer = tokio::time::interval(Duration::from_millis(50));
        let mut requested_cancel = false;
        let mut stop = None;
        loop {
            tokio::select! {
                _ = timer.tick(), if cancel_ledger.is_some() && !requested_cancel => {
                    if ledger(cancel_ledger.unwrap())?.iter().any(|entry| entry["event"] == "started" && entry["tool"] == "bridge_probe_wait") {
                        run.cancel()?;
                        requested_cancel = true;
                    }
                }
                event = run.next() => {
                    let Some(event) = event? else { break; };
                    match event {
                        AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                            if let ContentBlock::Text(text) = chunk.content { observed.text.push_str(&text.text); }
                        }
                        AcpEvent::Update(SessionUpdate::ToolCall(call)) => {
                            observed.tool_updates += 1;
                            tool_titles.insert(call.tool_call_id, call.title);
                        }
                        AcpEvent::Update(SessionUpdate::ToolCallUpdate(update)) => {
                            observed.tool_updates += 1;
                            if let Some(title) = update.fields.title { tool_titles.insert(update.tool_call_id, title); }
                        }
                        AcpEvent::Permission { id, request } if run.permission_pending(&id) => {
                            observed.permissions += 1;
                            // Only this disposable fixture's named tools are eligible for one-time approval.
                            // Permission subjects can be partial updates. Codex supplies
                            // the tool identity earlier, then references its ID here.
                            let title = request.tool_call.fields.title.as_deref()
                                .or_else(|| tool_titles.get(&request.tool_call.tool_call_id).map(String::as_str))
                                .unwrap_or_default();
                            let fixture = title.contains("bridge_probe_token") || title.contains("bridge_probe_wait");
                            observed.fixture_permissions += usize::from(fixture);
                            let option = if fixture && !deny { request.options.iter().find(|option| option.kind == PermissionOptionKind::AllowOnce).map(|option| option.option_id.to_string()) } else { None };
                            observed.allowed += usize::from(option.is_some());
                            run.respond(id, option.as_deref())?;
                        }
                        AcpEvent::Finished(reason) => stop = Some(reason),
                        _ => {}
                    }
                }
            }
        }
        observed.cancelled = requested_cancel && stop == Some(StopReason::Cancelled) && run.run().status() == RunStatus::Cancelled;
        if cancel_ledger.is_none() && (stop != Some(StopReason::EndTurn) || run.run().status() != RunStatus::Completed) {
            return Err(format!("prompt did not finish normally: {stop:?}").into());
        }
        Ok(observed)
    }).await.map_err(|_| "compatibility prompt timed out; no retry attempted")?
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let provider = args.next().ok_or("usage: provider_compat <provider> <tools|permissions|deny|cancel|resume|models> <absolute-workspace> [absolute-adapter.js]")?;
    let check = args.next().ok_or("missing check")?;
    if !matches!(
        check.as_str(),
        "tools" | "permissions" | "deny" | "cancel" | "resume" | "models"
    ) {
        return Err("unknown check".into());
    }
    let workspace = std::path::PathBuf::from(args.next().ok_or("missing workspace")?);
    if !workspace.is_absolute() || !workspace.is_dir() {
        return Err("workspace must be an existing absolute directory".into());
    }
    let definition = match provider.as_str() {
        "opencode" => ProviderDefinition::opencode(),
        "codex" => ProviderDefinition::codex(),
        "claude" => ProviderDefinition::claude(),
        _ => return Err("unknown provider".into()),
    };
    let mut profile = definition.profile("compatibility");
    if let Some(script) = args.next() {
        profile = profile.node_script(script);
    }
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    if provider == "codex"
        && let Ok(path) = std::env::var("AGENT_BRIDGE_CODEX_PATH")
    {
        profile = profile.env("CODEX_PATH", path);
    }
    if check == "models" {
        let first = std::env::var("AGENT_BRIDGE_MODEL_A")?;
        let second = std::env::var("AGENT_BRIDGE_MODEL_B")?;
        if first == second {
            return Err("choose two distinct models".into());
        }
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let state = std::env::temp_dir().join(format!(
        "agent-bridge-compat-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir(&state)?;
    let calls = state.join("calls.jsonl");
    let mut mcp = if matches!(check.as_str(), "tools" | "permissions" | "deny" | "cancel") {
        let node = std::env::var("AGENT_BRIDGE_NODE")
            .map_err(|_| "set AGENT_BRIDGE_NODE to an absolute Node executable for MCP checks")?;
        if !Path::new(&node).is_absolute() {
            return Err("AGENT_BRIDGE_NODE must be absolute".into());
        }
        vec![McpServer::Stdio(
            McpServerStdio::new("bridge-probe", node).args(vec![
                format!(
                    "{}/verification/adapters/probe-tools.mjs",
                    env!("CARGO_MANIFEST_DIR")
                ),
                calls.to_string_lossy().into_owned(),
            ]),
        )]
    } else {
        vec![]
    };
    if provider == "codex" && matches!(check.as_str(), "permissions" | "deny") {
        // ACP MCP attachment has no approval-policy field. For these two checks,
        // use the adapter's explicit native configuration override instead.
        let McpServer::Stdio(server) = &mcp[0] else {
            unreachable!()
        };
        let mut config: Value = match std::env::var("CODEX_CONFIG") {
            Ok(config) => serde_json::from_str(&config)?,
            Err(std::env::VarError::NotPresent) => json!({}),
            Err(error) => return Err(error.into()),
        };
        let config_object = config
            .as_object_mut()
            .ok_or("CODEX_CONFIG must be an object")?;
        let servers = config_object
            .entry("mcp_servers")
            .or_insert_with(|| json!({}));
        servers
            .as_object_mut()
            .ok_or("mcp_servers must be an object")?
            .insert(
                "bridge-probe".into(),
                json!({
                        "command":server.command,"args":server.args,
                "default_tools_approval_mode":"prompt"
                    }),
            );
        profile = profile.env("CODEX_CONFIG", serde_json::to_string(&config)?);
        mcp.clear();
    }
    let resolved = profile
        .inspect(&ExecutableSearch::current())
        .into_resolved()
        .map_err(|report| format!("setup required: {:?}", report.issues))?;
    let connection = AcpDriver.connect(resolved.clone()).await?;
    println!("Provider: {:?}", connection.report());
    println!("Evidence directory: {}", state.display());
    let result = async {
        let mut session = connection.new_session(SessionId::new(format!("compat-{unique}"))?, SlotId::new("compat")?, workspace, mcp).await?;
        if provider == "codex" && matches!(check.as_str(), "permissions" | "deny") {
            session.set_option("mode", ConfigValue::Select("read-only".into())).await?;
        }
        println!("Configuration: {:?}", session.configuration().values);
        if matches!(check.as_str(), "tools" | "permissions" | "deny" | "cancel") {
            let cancellation = check == "cancel";
            let input = if cancellation { "Call the MCP tool bridge_probe_wait exactly once. Do not use any other tool. Then repeat its returned token." } else { "Call the MCP tool bridge_probe_token exactly once. Do not use any other tool. Reply only with the exact token it returned." };
            let observed = prompt(&mut session, "tool-check", input, cancellation.then_some(calls.as_path()), check == "deny").await?;
            let entries = ledger(&calls)?;
            let token_matches = entries.iter().any(|entry| entry["event"] == "finished" && entry["tool"] == "bridge_probe_token" && entry["token"].as_str() == Some(observed.text.trim()));
            let passed = if check == "deny" {
                observed.fixture_permissions > 0 && observed.allowed == 0 && !entries.iter().any(|entry| entry["event"] == "started")
            } else {
                observed.tool_updates > 0 && if cancellation { observed.cancelled } else { token_matches && (check != "permissions" || observed.allowed > 0) }
            };
            let evidence = json!({"provider":provider,"check":check,"passed":passed,"permission_requests":observed.permissions,"fixture_permission_requests":observed.fixture_permissions,"allow_once_submitted":observed.allowed,"tool_updates":observed.tool_updates,"cancellation_confirmed":observed.cancelled,"token_matches":token_matches,"tool_ledger":entries});
            std::fs::write(state.join("result.json"), serde_json::to_vec_pretty(&evidence)?)?;
            println!("{evidence}");
            if !passed { return Err("workflow did not establish required evidence".into()); }
        } else {
            let marker = format!("bridge-continuity-{unique}");
            if check == "models" { session.set_model(std::env::var("AGENT_BRIDGE_MODEL_A").map_err(|_| "set AGENT_BRIDGE_MODEL_A and AGENT_BRIDGE_MODEL_B")?).await?; }
            let first_config = session.configuration().values;
            prompt(&mut session, "remember", &format!("Remember {marker}. Reply only remembered. Do not use tools."), None, false).await?;
            if check == "models" {
                let second = std::env::var("AGENT_BRIDGE_MODEL_B")?;
                if second == std::env::var("AGENT_BRIDGE_MODEL_A")? { return Err("choose two distinct models".into()); }
                session.set_model(second).await?;
                let second_config = session.configuration().values;
                let observed = prompt(&mut session, "recall", "What exact phrase did I ask you to remember? Reply only that phrase. Do not use tools.", None, false).await?;
                if observed.text.trim() != marker { return Err("model switch lost the expected phrase".into()); }
                std::fs::write(state.join("result.json"), serde_json::to_vec_pretty(&json!({"provider":provider,"check":check,"passed":true,"phrase_matches":true,"before":first_config,"after":second_config}))?)?;
                println!("Model continuity passed. Before: {first_config:?}; after: {second_config:?}");
            } else {
                let database = state.join("continuation.sqlite3");
                let store = SqliteStore::open(&database)?;
                let handoff = ContinuationId::new(format!("compat-{unique}"))?;
                session.handoff(handoff.clone(), &store)?;
                drop(store);
                return Ok(Some((database, handoff, marker)));
            }
        }
        Ok::<_, Box<dyn Error>>(None)
    }.await;
    let shutdown = connection.shutdown().await;
    let resume = result?;
    shutdown?;
    if let Some((database, handoff, marker)) = resume {
        let connection = AcpDriver.connect(resolved).await?;
        let result = async {
            let store = SqliteStore::open(database)?;
            let mut session = connection.acp().resume_saved(&store, &handoff, vec![]).await?;
            let observed = prompt(&mut session, "recall", "What exact phrase did I ask you to remember? Reply only that phrase. Do not use tools.", None, false).await?;
            if observed.text.trim() != marker { return Err("native resume lost the expected phrase".into()); }
            std::fs::write(state.join("result.json"), serde_json::to_vec_pretty(&json!({"provider":provider,"check":check,"passed":true,"phrase_matches":true,"sqlite_reopened":true,"provider_restarted":true}))?)?;
            println!("Native continuity passed across provider shutdown and SQLite reopen.");
            Ok::<_, Box<dyn Error>>(())
        }.await;
        let shutdown = connection.shutdown().await;
        result?;
        shutdown?;
    }
    Ok(())
}
