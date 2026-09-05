//! A real text prompt. Permission requests are dismissed by this example.
use std::{io::Write, time::Duration};

use agent_bridge::{
    RunId, SessionId, SlotId,
    acp::{AcpConnection, AcpEvent, AcpLaunch, ContentBlock, SessionUpdate},
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
        let mut run = session.start_run(RunId::new("example-run")?, prompt)?;
        while let Some(event) = run.next().await? {
            match event {
                AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                    if let ContentBlock::Text(text) = chunk.content {
                        print!("{}", text.text);
                        std::io::stdout().flush()?;
                    }
                }
                AcpEvent::Permission { id, request } => {
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
    Ok(())
}
