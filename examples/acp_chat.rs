//! A real text prompt. Permission requests are dismissed by this example.
use std::{io::Write, time::Duration};

use agent_bridge::{
    ActorId, RunId, SessionId, SlotId,
    acp::{AcpConnection, AcpEvent, AcpLaunch, ContentBlock, RecordActors, SessionUpdate},
    records::{MemoryStore, RecordStore},
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let cwd = args
        .next()
        .ok_or("usage: acp_chat <absolute-workspace> <prompt> <executable> [args...]")?;
    let prompt = args.next().ok_or("missing prompt")?;
    let executable = args.next().ok_or("missing ACP executable")?;
    let mut launch = AcpLaunch::new(executable);
    for argument in args {
        launch = launch.arg(argument);
    }
    let connection = AcpConnection::connect(launch).await?;
    let store = MemoryStore::default();

    // The limit covers session setup, generation, and any stalled provider.
    // Dropping this scope retires unfinished work before connection shutdown.
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        let mut session = connection
            .new_session(
                SessionId::new("example-chat")?,
                SlotId::new("example-agent")?,
                cwd,
                vec![],
            )
            .await?;
        let mut run = session.start_recorded_run(
            RunId::new("example-run")?,
            prompt,
            &store,
            RecordActors {
                user: ActorId::new("user")?,
                agent: ActorId::new("assistant")?,
                host: ActorId::new("example-app")?,
            },
        )?;
        while let Some(event) = run.next().await? {
            match event {
                AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                    if let ContentBlock::Text(text) = chunk.content {
                        print!("{}", text.text);
                        std::io::stdout().flush()?;
                    }
                }
                AcpEvent::Permission { id, request } => {
                    if !run.permission_pending(&id) {
                        continue;
                    }
                    eprintln!(
                        "\nDismissing permission request: {:?}",
                        request.tool_call.fields.title
                    );
                    run.respond(id, None)?;
                }
                AcpEvent::Finished(reason) => println!("\nStopped: {reason:?}"),
                _ => {}
            }
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await;
    let shutdown = connection.shutdown().await;
    outcome??;
    shutdown?;
    let history = store.list(&SessionId::new("example-chat")?, None, 1000)?;
    println!(
        "{} portable records remain available after connection shutdown.",
        history.len()
    );
    Ok(())
}
