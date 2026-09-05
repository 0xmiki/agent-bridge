//! ACP v1 subprocess sessions, text prompts, and permission routing.
//!
//! Requires a running Tokio runtime. Launch arguments are passed directly, without
//! shell parsing. The child inherits the host working directory and environment.
//! Each session specifies its own absolute working directory.
//!
//! No filesystem or terminal capabilities are advertised. This module does not
//! log wire messages, install agents, or authenticate automatically.

mod configuration;
mod context;
pub use context::{ContextTask, TextContextMode};
mod continuation;
mod recording;
mod session;
pub use recording::{RecordActors, RecordedRun, RecordingError};
pub use session::{AcpEvent, AcpRun, AcpSession, PermissionId};

use std::{
    error::Error,
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo,
    schema::{
        ProtocolVersion,
        v1::{
            BooleanConfigOptionCapabilities, ClientCapabilities, ClientSessionCapabilities,
            Implementation, InitializeRequest, SessionConfigOptionsCapabilities,
        },
    },
};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

/// Raw advertised capabilities. These describe ACP support, not verified behavior.
/// SDK types are exposed only within this protocol-specific module.
pub use agent_client_protocol::schema::v1::InitializeResponse as AcpInfo;
pub use agent_client_protocol::schema::v1::{
    ContentBlock, McpServer, McpServerHttp, McpServerSse, McpServerStdio, SessionUpdate, StopReason,
};

/// Launch configuration for one installed ACP agent.
///
/// This intentionally has no `Debug` implementation that could expose environment
/// values. It does not install missing binaries or parse shell command strings.
#[derive(Clone)]
pub struct AcpLaunch {
    process: AcpAgentConfig,
    initialize_timeout: Duration,
    session_timeout: Duration,
    continuation_scope: Option<String>,
}

impl AcpLaunch {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            process: AcpAgentConfig::new(executable),
            initialize_timeout: Duration::from_secs(15),
            session_timeout: Duration::from_secs(30),
            continuation_scope: None,
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

    /// Maximum wait for new or resumed session setup. A timeout never retries.
    #[must_use]
    pub fn session_timeout(mut self, duration: Duration) -> Self {
        self.session_timeout = duration;
        self
    }

    /// Stable application-owned account/profile/environment namespace for handoffs.
    /// Change it when switching credentials or provider state directories.
    #[must_use]
    pub fn continuation_scope(mut self, scope: impl Into<String>) -> Self {
        self.continuation_scope = Some(scope.into());
        self
    }
}

#[derive(Debug)]
pub enum AcpError {
    InitializationTimedOut,
    ClosedBeforeInitialization,
    UnsupportedProtocolVersion,
    ShutdownTimedOut,
    RequestTimedOut,
    Closed,
    InvalidWorkingDirectory,
    InvalidMcpCommand,
    UnsupportedMcpTransport,
    EmptyPrompt,
    SessionUnavailable,
    InvalidPermission,
    EventBufferFull,
    ResumeUnsupported,
    ContinuationScopeRequired,
    IncompatibleContinuation,
    UnsafeHandoff,
    ConfigurationUnsupported,
    UnknownConfigurationOption,
    InvalidConfigurationValue,
    InvalidConfiguration,
    ConfigurationUncertain,
    ConfigurationRejected,
    ConfigurationChanged,
    ModelSelectorUnavailable,
    Store(crate::records::StoreError),
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
            Self::RequestTimedOut => {
                f.write_str("ACP session setup timed out; no retry was attempted")
            }
            Self::Closed => f.write_str("ACP connection closed; execution outcome may be unknown"),
            Self::InvalidWorkingDirectory => {
                f.write_str("ACP requires an absolute working directory")
            }
            Self::InvalidMcpCommand => f.write_str("ACP requires an absolute MCP executable path"),
            Self::UnsupportedMcpTransport => {
                f.write_str("agent does not advertise the requested MCP transport")
            }
            Self::EmptyPrompt => f.write_str("prompt cannot be blank"),
            Self::SessionUnavailable => {
                f.write_str("session is active, retired, or returned a duplicate provider ID")
            }
            Self::InvalidPermission => {
                f.write_str("permission is resolved, unknown, or the option was not offered")
            }
            Self::EventBufferFull => {
                f.write_str("ACP event buffer filled; this run cannot be consumed reliably")
            }
            Self::ResumeUnsupported => {
                f.write_str("agent does not advertise native session resume")
            }
            Self::ContinuationScopeRequired => {
                f.write_str("set a nonempty continuation scope for this provider connection")
            }
            Self::IncompatibleContinuation => f.write_str(
                "continuation is incompatible with this scope, adapter, or agent version",
            ),
            Self::UnsafeHandoff => {
                f.write_str("session has unfinished or uncertain work and cannot be handed off")
            }
            Self::ConfigurationUnsupported => {
                f.write_str("provider did not expose session configuration options")
            }
            Self::UnknownConfigurationOption => {
                f.write_str("configuration option was not offered by the provider")
            }
            Self::InvalidConfigurationValue => {
                f.write_str("value has the wrong type or was not offered for this option")
            }
            Self::InvalidConfiguration => {
                f.write_str("provider returned an invalid configuration catalog")
            }
            Self::ConfigurationUncertain => {
                f.write_str("configuration is pending or uncertain; wait for a report or reconnect")
            }
            Self::ConfigurationRejected => {
                f.write_str("provider did not confirm the requested option value")
            }
            Self::ConfigurationChanged => f.write_str(
                "configuration changed before dispatch; choose a new run ID and retry explicitly",
            ),
            Self::ModelSelectorUnavailable => f.write_str(
                "provider does not expose exactly one model selector; use an explicit option ID",
            ),
            Self::Store(error) => error.fmt(f),
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
            Self::Store(error) => Some(error),
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
    connection: ConnectionTo<Agent>,
    routes: Arc<Mutex<session::Routes>>,
    closed: watch::Receiver<bool>,
    continuation_scope: Option<String>,
    session_timeout: Duration,
}

impl AcpConnection {
    pub async fn connect(launch: AcpLaunch) -> Result<Self, AcpError> {
        let continuation_scope = launch.continuation_scope.clone();
        let session_timeout = launch.session_timeout;
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel();
        let (closed_tx, closed_rx) = watch::channel(false);
        let (fault_tx, mut fault_rx) = watch::channel(false);
        let routes = Arc::new(Mutex::new(session::Routes::new(fault_tx)));
        let updates = routes.clone();
        let permissions = routes.clone();
        let handle = tokio::spawn(async move {
            // Dropping the sender also wakes watchers when this task is aborted.
            let _closed_tx = closed_tx;
            Client
                .builder()
                .name("agent-bridge")
                .on_receive_notification(
                    async move |notification, _cx| session::route_update(&updates, notification),
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request, responder, _cx| {
                        session::route_permission(&permissions, request, responder)
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(AcpAgent::new(launch.process), async move |connection| {
                    let info = connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1).client_info(
                            Implementation::new("agent-bridge", env!("CARGO_PKG_VERSION")),
                        ).client_capabilities(ClientCapabilities::new().session(
                            ClientSessionCapabilities::new().config_options(
                                SessionConfigOptionsCapabilities::new().boolean(BooleanConfigOptionCapabilities::new())
                            )
                        )))
                        .block_task()
                        .await?;
                    // The receiving owner may have cancelled its connect future.
                    if ready_tx.send((info, connection.clone())).is_ok() {
                        tokio::select! {
                            _ = stop_rx => {},
                            _ = fault_rx.changed() => return Err(agent_client_protocol::Error::internal_error()),
                        }
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

        let (info, connection) = match timeout(launch.initialize_timeout, ready_rx).await {
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
        Ok(Self {
            info,
            task,
            connection,
            routes,
            closed: closed_rx,
            continuation_scope,
            session_timeout,
        })
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
