//! Initialize an installed ACP agent without creating a session or calling a model.
use agent_bridge::acp::{AcpConnection, AcpLaunch};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args();
    let _ = args.next();
    let executable = args
        .next()
        .ok_or("usage: cargo run --example acp_connect -- <executable> [arguments...]")?;
    let mut launch = AcpLaunch::new(executable);
    for argument in args {
        launch = launch.arg(argument);
    }

    let connection = AcpConnection::connect(launch).await?;
    println!("Agent: {:?}", connection.info().agent_info);
    println!("Capabilities: {:?}", connection.info().agent_capabilities);
    connection.shutdown().await?;
    println!("Connection closed.");
    Ok(())
}
