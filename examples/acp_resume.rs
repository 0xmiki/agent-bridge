//! Hand off one native session, restart its agent process, and resume it.
use std::{
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, AcpSession, ContentBlock, SessionUpdate,
};
use agent_bridge::records::SqliteStore;
use agent_bridge::{ContinuationId, RunId, SessionId, SlotId};

async fn prompt(
    session: &mut AcpSession<'_>,
    run_id: RunId,
    text: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut output = String::new();
    let mut run = session.start_run(run_id, text)?;
    while let Some(event) = run.next().await? {
        match event {
            AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                if let ContentBlock::Text(text) = chunk.content {
                    print!("{}", text.text);
                    std::io::stdout().flush()?;
                    output.push_str(&text.text);
                }
            }
            AcpEvent::Permission { id, request } => {
                eprintln!(
                    "\nDismissing permission request: {:?}",
                    request.tool_call.fields.title
                );
                if run.permission_pending(&id) {
                    run.respond(id, None)?;
                }
            }
            AcpEvent::Finished(reason) => println!("\nStopped: {reason:?}"),
            _ => {}
        }
    }
    Ok(output)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let database = args
        .next()
        .ok_or("usage: acp_resume <database-path> <absolute-workspace> <executable> [args...]")?;
    let workspace = args.next().ok_or("missing workspace")?;
    let executable = args.next().ok_or("missing ACP executable")?;
    let mut launch = AcpLaunch::new(executable).continuation_scope("example/default");
    for argument in args {
        launch = launch.arg(argument.to_string_lossy());
    }

    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let app_session = SessionId::new(format!("resume-example-{unique}"))?;
    let slot = SlotId::new("example-agent")?;
    let first_handoff = ContinuationId::new(format!("handoff-{unique}-1"))?;
    let second_handoff = ContinuationId::new(format!("handoff-{unique}-2"))?;
    let phrase = format!("bridge-memory-{unique}");

    let store = SqliteStore::open(&database)?;
    let first = AcpConnection::connect(launch.clone()).await?;
    let mut session = first
        .new_session(app_session, slot, workspace, vec![])
        .await?;
    println!("First process:");
    prompt(
        &mut session,
        RunId::new(format!("run-{unique}-1"))?,
        &format!(
            "Remember the exact phrase {phrase} for my next turn. Reply only with 'remembered'. Do not use tools."
        ),
    )
    .await?;
    session.handoff(first_handoff.clone(), &store)?;
    first.shutdown().await?;
    drop(store);

    let store = SqliteStore::open(&database)?;
    let second = AcpConnection::connect(launch).await?;
    let mut session = second.resume_saved(&store, &first_handoff, vec![]).await?;
    println!("Second process:");
    let answer = prompt(
        &mut session,
        RunId::new(format!("run-{unique}-2"))?,
        "What exact phrase did I ask you to remember? Reply with only that phrase. Do not use tools.",
    )
    .await?;
    session.handoff(second_handoff, &store)?;
    second.shutdown().await?;

    if answer.trim() != phrase {
        return Err(format!(
            "resumed agent returned {:?}; expected {:?}",
            answer.trim(),
            phrase
        )
        .into());
    }
    println!("Native context survived the process restart.");
    Ok(())
}
