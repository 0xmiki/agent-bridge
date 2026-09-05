//! ACP v1 subprocess initialization, separate from the core execution model.
//!
//! Requires a running Tokio runtime. Launch arguments are passed directly, without
//! shell parsing. The child inherits the host working directory and environment.
//! Workspace selection belongs to session creation, which is not implemented yet.
//!
//! No filesystem or terminal capabilities are advertised. This module does not
//! log wire messages, install agents, authenticate, create sessions, or send prompts.

use std::{error::Error, fmt, path::PathBuf, time::Duration};

use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Client,
    schema::{
        ProtocolVersion,
        v1::{Implementation, InitializeRequest},
    },
};
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

/// Raw advertised capabilities. These describe ACP support, not verified behavior.
/// SDK types are exposed only within this protocol-specific module.
pub use agent_client_protocol::schema::v1::InitializeResponse as AcpInfo;

/// Launch configuration for one installed ACP agent.
///
/// This intentionally has no `Debug` implementation that could expose environment
/// values. It does not install missing binaries or parse shell command strings.
#[derive(Clone)]
pub struct AcpLaunch {
    process: AcpAgentConfig,
    initialize_timeout: Duration,
}

impl AcpLaunch {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            process: AcpAgentConfig::new(executable),
            initialize_timeout: Duration::from_secs(15),
        }
    }

    #[must_use]
    pub fn arg(mut self, argument: impl Into<String>) -> Self {
        self.process = self.process.arg(argument);
        self
    }

    #[must_use]
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.process = self.process.env(name, value);
        self
    }

    #[must_use]
    pub fn initialize_timeout(mut self, duration: Duration) -> Self {
        self.initialize_timeout = duration;
        self
    }
}

#[derive(Debug)]
pub enum AcpError {
    InitializationTimedOut,
    ClosedBeforeInitialization,
    UnsupportedProtocolVersion,
    ShutdownTimedOut,
    /// May include the SDK's bounded child stderr diagnostics. Do not log blindly.
    Protocol(agent_client_protocol::Error),
    Task(tokio::task::JoinError),
}

impl fmt::Display for AcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitializationTimedOut => f.write_str("ACP initialization timed out"),
            Self::ClosedBeforeInitialization => f.write_str("ACP closed before initialization"),
            Self::UnsupportedProtocolVersion => f.write_str("agent did not negotiate ACP v1"),
            Self::ShutdownTimedOut => f.write_str("ACP shutdown timed out"),
            Self::Protocol(error) => write!(f, "ACP connection failed: {error}"),
            Self::Task(error) => write!(f, "ACP connection task failed: {error}"),
        }
    }
}

impl Error for AcpError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::Task(error) => Some(error),
            _ => None,
        }
    }
}

/// An initialized connection that owns its provider process lifetime.
///
/// Use `shutdown().await` for explicit cleanup. Dropping the connection or its
/// pending connect future aborts its task, triggering SDK process cleanup when
/// the executor next polls it. Keep the Tokio runtime alive through cleanup.
/// The SDK terminates process groups on Unix; Windows descendant cleanup has
/// not been established by this implementation.
pub struct AcpConnection {
    info: AcpInfo,
    task: ConnectionTask,
}

impl AcpConnection {
    pub async fn connect(launch: AcpLaunch) -> Result<Self, AcpError> {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            Client
                .builder()
                .name("agent-bridge")
                .connect_with(AcpAgent::new(launch.process), async move |connection| {
                    let info = connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1).client_info(
                            Implementation::new("agent-bridge", env!("CARGO_PKG_VERSION")),
                        ))
                        .block_task()
                        .await?;
                    // The receiving owner may have cancelled its connect future.
                    if ready_tx.send(info).is_ok() {
                        let _ = stop_rx.await;
                    }
                    Ok(())
                })
                .await
                .map_err(AcpError::Protocol)
        });
        let mut task = ConnectionTask {
            handle,
            stop: Some(stop_tx),
        };

        let info = match timeout(launch.initialize_timeout, ready_rx).await {
            Ok(Ok(info)) => info,
            Ok(Err(_)) => {
                task.wait().await?;
                return Err(AcpError::ClosedBeforeInitialization);
            }
            Err(_) => {
                task.abort_and_wait().await;
                return Err(AcpError::InitializationTimedOut);
            }
        };
        if info.protocol_version != ProtocolVersion::V1 {
            task.abort_and_wait().await;
            return Err(AcpError::UnsupportedProtocolVersion);
        }
        Ok(Self { info, task })
    }

    pub fn info(&self) -> &AcpInfo {
        &self.info
    }

    /// A local task snapshot, not a liveness probe of the remote agent.
    pub fn is_closed(&self) -> bool {
        self.task.handle.is_finished()
    }

    /// Observe spontaneous closure or failure. Cancelling this future closes the connection.
    pub async fn wait_closed(mut self) -> Result<(), AcpError> {
        self.task.wait().await
    }

    /// End the protocol scope and wait for the SDK's bounded process cleanup.
    pub async fn shutdown(mut self) -> Result<(), AcpError> {
        if let Some(stop) = self.task.stop.take() {
            let _ = stop.send(());
        }
        match timeout(Duration::from_secs(3), self.task.wait()).await {
            Ok(result) => result,
            Err(_) => {
                self.task.abort_and_wait().await;
                Err(AcpError::ShutdownTimedOut)
            }
        }
    }
}

struct ConnectionTask {
    handle: JoinHandle<Result<(), AcpError>>,
    stop: Option<oneshot::Sender<()>>,
}

impl ConnectionTask {
    async fn wait(&mut self) -> Result<(), AcpError> {
        (&mut self.handle).await.map_err(AcpError::Task)?
    }

    async fn abort_and_wait(&mut self) {
        self.handle.abort();
        let _ = (&mut self.handle).await;
    }
}

impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
