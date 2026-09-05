use super::{ACP_STDIO_DRIVER, ResolvedProvider, SetupAction};
use crate::acp::{AcpConnection, AcpError, AcpLaunch, AcpSession, McpServer};
use crate::{SessionId, SlotId};
use std::{
    error::Error, fmt, future::Future, path::PathBuf, process::Stdio, sync::Mutex, time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

/// Driver implementations consume the same resolved launch description. The
/// associated connection keeps driver-specific APIs explicit until the host SDK
/// contract is introduced. Built-in providers currently share AcpDriver.
pub trait ProviderDriver: Send + Sync {
    type Connection;
    fn id(&self) -> &str;
    fn connect(
        &self,
        provider: ResolvedProvider,
    ) -> impl Future<Output = Result<Self::Connection, ProviderError>> + Send;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationState {
    Unknown,
    Required,
}

#[derive(Debug, Clone)]
pub struct AdvertisedCapabilities {
    pub image_input: bool,
    pub embedded_context: bool,
    pub load_session: bool,
    pub resume_session: bool,
    pub mcp_http: bool,
    pub mcp_sse: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderReport {
    pub provider: String,
    pub driver: String,
    pub reported_name: Option<String>,
    pub reported_version: Option<String>,
    pub authentication: AuthenticationState,
    pub auth_methods: Vec<(String, String)>,
    /// Protocol declarations, not results of workflow compatibility tests.
    pub advertised: AdvertisedCapabilities,
}

#[derive(Debug)]
pub enum ProviderError {
    UnsupportedDriver(String),
    UnsupportedRuntime {
        minimum_major: u32,
        found_major: Option<u32>,
    },
    RuntimeProbeTimedOut,
    RuntimeProbeFailed,
    IncompatibleProtocol,
    AuthenticationRequired,
    Agent(AcpError),
}
impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDriver(id) => write!(f, "no matching driver for {id}"),
            Self::UnsupportedRuntime {
                minimum_major,
                found_major,
            } => write!(
                f,
                "Node {minimum_major}+ is required; reported major version: {found_major:?}"
            ),
            Self::RuntimeProbeTimedOut => f.write_str("Node version probe timed out"),
            Self::RuntimeProbeFailed => f.write_str("Node version probe failed"),
            Self::IncompatibleProtocol => {
                f.write_str("provider negotiated an unsupported ACP protocol version")
            }
            Self::AuthenticationRequired => {
                f.write_str("provider requires authentication; use its supported local setup flow")
            }
            Self::Agent(error) => error.fmt(f),
        }
    }
}
impl Error for ProviderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Agent(error) => Some(error),
            _ => None,
        }
    }
}
impl From<AcpError> for ProviderError {
    fn from(error: AcpError) -> Self {
        match error {
            AcpError::UnsupportedProtocolVersion => Self::IncompatibleProtocol,
            AcpError::Protocol(ref error)
                if error.code == agent_client_protocol::ErrorCode::AuthRequired =>
            {
                Self::AuthenticationRequired
            }
            other => Self::Agent(other),
        }
    }
}

#[derive(Default)]
pub struct AcpDriver;

impl ProviderDriver for AcpDriver {
    type Connection = ConnectedProvider;
    fn id(&self) -> &str {
        ACP_STDIO_DRIVER
    }

    async fn connect(&self, provider: ResolvedProvider) -> Result<Self::Connection, ProviderError> {
        if provider.definition.driver != self.id() {
            return Err(ProviderError::UnsupportedDriver(provider.definition.driver));
        }
        if let (Some(node), Some(minimum)) = (
            &provider.node_runtime,
            provider.definition.minimum_node_major,
        ) {
            check_node(node, minimum, &provider.launch.env).await?;
        }
        let mut launch =
            AcpLaunch::new(provider.launch.executable).continuation_scope(provider.launch.scope);
        for arg in provider.launch.args {
            launch = launch.arg(arg);
        }
        for (key, value) in provider.launch.env {
            launch = launch.env(key, value);
        }
        let connection = AcpConnection::connect(launch)
            .await
            .map_err(ProviderError::from)?;
        Ok(ConnectedProvider {
            provider: provider.definition.id,
            setup: provider.definition.setup,
            connection,
            authentication: Mutex::new(AuthenticationState::Unknown),
        })
    }
}

/// Successful initialization. Authentication and model access may still be unknown.
pub struct ConnectedProvider {
    provider: String,
    setup: Vec<SetupAction>,
    connection: AcpConnection,
    authentication: Mutex<AuthenticationState>,
}

impl ConnectedProvider {
    pub fn report(&self) -> ProviderReport {
        let info = self.connection.info();
        let capabilities = &info.agent_capabilities;
        ProviderReport {
            provider: self.provider.clone(),
            driver: ACP_STDIO_DRIVER.into(),
            reported_name: info.agent_info.as_ref().map(|info| info.name.clone()),
            reported_version: info.agent_info.as_ref().map(|info| info.version.clone()),
            authentication: *self.authentication.lock().unwrap(),
            auth_methods: info
                .auth_methods
                .iter()
                .map(|method| (method.id().to_string(), method.name().to_owned()))
                .collect(),
            advertised: AdvertisedCapabilities {
                image_input: capabilities.prompt_capabilities.image,
                embedded_context: capabilities.prompt_capabilities.embedded_context,
                load_session: capabilities.load_session,
                resume_session: capabilities.session_capabilities.resume.is_some(),
                mcp_http: capabilities.mcp_capabilities.http,
                mcp_sse: capabilities.mcp_capabilities.sse,
            },
        }
    }
    pub fn setup(&self) -> &[SetupAction] {
        &self.setup
    }

    pub async fn new_session(
        &self,
        session: SessionId,
        slot: SlotId,
        cwd: impl Into<PathBuf>,
        mcp: Vec<McpServer>,
    ) -> Result<AcpSession<'_>, ProviderError> {
        match self.connection.new_session(session, slot, cwd, mcp).await {
            Ok(session) => Ok(session),
            Err(error) => Err(self.classify_error(error)),
        }
    }

    /// Classify an error from a session/run using structured protocol codes only.
    pub fn classify_error(&self, error: AcpError) -> ProviderError {
        let error = ProviderError::from(error);
        if matches!(error, ProviderError::AuthenticationRequired) {
            *self.authentication.lock().unwrap() = AuthenticationState::Required;
        }
        error
    }

    /// Driver-specific access for continuation and configuration APIs.
    pub fn acp(&self) -> &AcpConnection {
        &self.connection
    }
    pub async fn shutdown(self) -> Result<(), ProviderError> {
        self.connection.shutdown().await.map_err(Into::into)
    }
}

async fn check_node(
    node: &std::path::Path,
    minimum: u32,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<(), ProviderError> {
    let mut child = Command::new(node)
        .arg("--version")
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ProviderError::RuntimeProbeFailed)?;
    let result = timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or(ProviderError::RuntimeProbeFailed)?
            .take(128)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ProviderError::RuntimeProbeFailed)?;
        let status = child
            .wait()
            .await
            .map_err(|_| ProviderError::RuntimeProbeFailed)?;
        if !status.success() {
            return Err(ProviderError::RuntimeProbeFailed);
        }
        let found = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| text.trim().strip_prefix('v'))
            .and_then(|text| text.split('.').next())
            .and_then(|major| major.parse::<u32>().ok());
        if found.is_none_or(|major| major < minimum) {
            return Err(ProviderError::UnsupportedRuntime {
                minimum_major: minimum,
                found_major: found,
            });
        }
        Ok(())
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            Err(ProviderError::RuntimeProbeTimedOut)
        }
    }
}
