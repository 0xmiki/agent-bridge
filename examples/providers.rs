//! Inspect built-in providers without running commands, or explicitly probe one.
use agent_bridge::acp::{AcpEvent, ContentBlock, SessionUpdate};
use agent_bridge::providers::{AcpDriver, ExecutableSearch, ProviderDefinition, ProviderDriver};
use agent_bridge::{RunId, SessionId, SlotId};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let requested = args.next();
    let script = args.next();
    if requested
        .as_deref()
        .is_some_and(|name| !matches!(name, "opencode" | "codex" | "claude"))
        || args.next().is_some()
    {
        return Err("usage: providers [opencode|codex|claude [absolute-node-script]]".into());
    }
    let mut failed = false;
    let search = ExecutableSearch::current();
    for definition in [
        ProviderDefinition::opencode(),
        ProviderDefinition::codex(),
        ProviderDefinition::claude(),
    ] {
        if requested
            .as_ref()
            .is_some_and(|name| name != &definition.id)
        {
            continue;
        }
        let mut profile = definition.profile("example-default");
        if let Some(script) = &script {
            profile = profile.node_script(script);
        }
        // Development-only overrides are explicit and their values are not printed.
        if let Ok(path) = std::env::var("AGENT_BRIDGE_CODEX_PATH")
            && profile.definition().id == "codex"
        {
            profile = profile.env("CODEX_PATH", path);
        }
        let inspected = profile.inspect(&search);
        println!(
            "{}: {}",
            inspected.report.provider,
            if inspected.is_launchable() {
                "launchable; authentication unknown"
            } else {
                "setup required"
            }
        );
        for issue in &inspected.report.issues {
            println!("  {issue:?}");
        }
        if requested.is_none() || !inspected.is_launchable() {
            failed |= requested.is_some();
            continue;
        }
        let connection = AcpDriver
            .connect(
                inspected
                    .into_resolved()
                    .map_err(|report| format!("{:?}", report.issues))?,
            )
            .await?;
        println!("{:?}", connection.report());
        if let Ok(workspace) = std::env::var("AGENT_BRIDGE_PROBE_WORKSPACE") {
            let result = connection
                .new_session(
                    SessionId::new("provider-probe")?,
                    SlotId::new("provider-probe-slot")?,
                    workspace,
                    vec![],
                )
                .await;
            match result {
                Ok(mut session) => {
                    println!(
                        "Session opened; configuration: {:?}",
                        session.configuration().values
                    );
                    if let Ok(prompt) = std::env::var("AGENT_BRIDGE_PROBE_PROMPT") {
                        let workflow =
                            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                                let mut run = session
                                    .start_run(RunId::new("provider-probe-run").unwrap(), prompt)?;
                                let mut text = String::new();
                                while let Some(event) = run.next().await? {
                                    match event {
                                        AcpEvent::Update(SessionUpdate::AgentMessageChunk(
                                            chunk,
                                        )) => {
                                            if let ContentBlock::Text(chunk) = chunk.content {
                                                text.push_str(&chunk.text);
                                            }
                                        }
                                        AcpEvent::Permission { id, .. }
                                            if run.permission_pending(&id) =>
                                        {
                                            run.respond(id, None)?
                                        }
                                        _ => {}
                                    }
                                }
                                Ok::<_, agent_bridge::acp::AcpError>(text)
                            })
                            .await;
                        match workflow {
                            Ok(Ok(text)) => println!("Prompt completed: {}", text.trim()),
                            Ok(Err(error)) => {
                                failed = true;
                                println!("Prompt failed: {}", connection.classify_error(error))
                            }
                            Err(_) => {
                                failed = true;
                                println!("Prompt timed out; no retry was attempted.");
                            }
                        }
                    }
                    drop(session);
                }
                Err(error) => {
                    failed = true;
                    println!("Session setup: {error}");
                }
            }
        }
        println!(
            "Authentication evidence: {:?}",
            connection.report().authentication
        );
        connection.shutdown().await?;
    }
    if failed {
        Err("provider probe did not complete; see diagnostics above".into())
    } else {
        Ok(())
    }
}
