use super::*;
use agent_bridge::acp::McpServerStdio;
use agent_bridge::mcp::McpToolServer;
use agent_client_protocol::schema::v1::EnvVariable;
use rmcp::ServiceExt;
use std::{io::Read, os::unix::fs::DirBuilderExt, path::PathBuf};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

pub(super) struct Transport {
    task: tokio::task::JoinHandle<()>,
    directory: PathBuf,
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
pub(super) async fn start(
    registry: Arc<ToolRegistry>,
    grant: ToolGrant,
    state: Arc<Bound>,
) -> Result<(Transport, McpServer), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut random = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut random))
        .map_err(|e| e.to_string())?;
    let capability: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let directory = std::env::temp_dir().join(format!("ab-tools-{}", &capability[..16]));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|e| e.to_string())?;
    let path = directory.join("mcp.sock");
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = std::fs::remove_dir(&directory);
            return Err(error.to_string());
        }
    };
    let mcp = McpServer::Stdio(
        McpServerStdio::new("application", executable)
            .args(vec!["--mcp-tools".into()])
            .env(vec![
                EnvVariable::new("AGENT_BRIDGE_MCP_ENDPOINT", path.to_string_lossy()),
                EnvVariable::new("AGENT_BRIDGE_MCP_CAPABILITY", capability.clone()),
            ]),
    );
    let task = tokio::spawn(async move {
        let capacity = Arc::new(Semaphore::new(4));
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = state.lifetime.cancelled() => break,
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
                accepted = listener.accept() => {
                    let Ok((stream,_)) = accepted else { break; };
                    let Ok(permit) = capacity.clone().try_acquire_owned() else { drop(stream); continue; };
                    let capability = capability.clone(); let registry = registry.clone(); let grant = grant.clone();
                    tasks.spawn(async move {
                        let _permit = permit;
                        let (read, mut write) = stream.into_split(); let mut read = BufReader::new(read);
                        let mut hello = String::new();
                        let read_hello = tokio::time::timeout(Duration::from_secs(2), (&mut read).take(512).read_line(&mut hello)).await;
                        if !matches!(read_hello,Ok(Ok(n)) if n > 0) || !hello.ends_with('\n') { return; }
                        #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Hello { capability:String }
                        if !serde_json::from_str::<Hello>(&hello).is_ok_and(|h| h.capability == capability) { return; }
                        if write.write_all(b"ok\n").await.is_err() { return; }
                        let server = McpToolServer::new(registry,grant.clone(),grant.scope.clone(),grant.subject.clone(),grant.issuer.clone()).unwrap();
                        if let Ok(service) = server.serve((read,write)).await { let _ = service.waiting().await; }
                    });
                }
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    });
    Ok((Transport { task, directory }, mcp))
}

pub(super) async fn helper() -> Result<(), String> {
    let path = std::env::var("AGENT_BRIDGE_MCP_ENDPOINT").map_err(|_| "missing MCP endpoint")?;
    let capability =
        std::env::var("AGENT_BRIDGE_MCP_CAPABILITY").map_err(|_| "missing MCP capability")?;
    let mut stream = UnixStream::connect(path).await.map_err(|e| e.to_string())?;
    stream
        .write_all(format!("{}\n", json!({"capability":capability})).as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut ack = [0; 3];
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut ack))
        .await
        .map_err(|_| "MCP authentication timed out")?
        .map_err(|e| e.to_string())?;
    if &ack != b"ok\n" {
        return Err("MCP authentication rejected".into());
    }
    let (mut from_host, mut to_host) = stream.into_split();
    let input = async { tokio::io::copy(&mut tokio::io::stdin(), &mut to_host).await };
    let output = async { tokio::io::copy(&mut from_host, &mut tokio::io::stdout()).await };
    tokio::select! { result=input=>result.map_err(|e|e.to_string())?, result=output=>result.map_err(|e|e.to_string())? };
    Ok(())
}
