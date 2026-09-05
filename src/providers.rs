//! Installed-provider definitions, read-only discovery, and explicit driver probes.
mod discovery;
mod driver;
pub use discovery::{
    ExecutableSearch, Inspection, InstallationReport, ResolvedProvider, SetupIssue,
};
pub use driver::{
    AcpDriver, AdvertisedCapabilities, AuthenticationState, ConnectedProvider, ProviderDriver,
    ProviderError, ProviderReport,
};

use std::{collections::BTreeMap, path::PathBuf};

/// The shared built-in transport. Custom drivers may use their own identifiers.
pub const ACP_STDIO_DRIVER: &str = "acp-stdio";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupAction {
    pub description: String,
    /// Guidance only. Discovery and connection never execute these commands.
    pub command: Option<String>,
    pub documentation: String,
}

#[derive(Debug, Clone)]
pub struct ProviderDefinition {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub command: String,
    pub args: Vec<String>,
    pub standalone_cli: Option<String>,
    pub adapter_required: bool,
    pub minimum_node_major: Option<u32>,
    pub setup: Vec<SetupAction>,
}

impl ProviderDefinition {
    pub fn custom(id: impl Into<String>, command: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            driver: ACP_STDIO_DRIVER.into(),
            command: command.into(),
            args: vec![],
            standalone_cli: None,
            adapter_required: false,
            minimum_node_major: None,
            setup: vec![],
        }
    }

    pub fn opencode() -> Self {
        Self {
            id: "opencode".into(),
            name: "OpenCode".into(),
            driver: ACP_STDIO_DRIVER.into(),
            command: "opencode".into(),
            args: vec!["acp".into()],
            standalone_cli: Some("opencode".into()),
            adapter_required: false,
            minimum_node_major: None,
            setup: vec![SetupAction {
                description: "Install OpenCode and authenticate a model provider.".into(),
                command: Some("opencode auth login".into()),
                documentation: "https://opencode.ai/docs/".into(),
            }],
        }
    }

    pub fn codex() -> Self {
        Self { id: "codex".into(), name: "Codex".into(), driver: ACP_STDIO_DRIVER.into(), command: "codex-acp".into(),
            args: vec![], standalone_cli: Some("codex".into()), adapter_required: true, minimum_node_major: None,
            setup: vec![
                SetupAction { description: "Install the Codex ACP adapter. Its npm package includes a compatible Codex runtime.".into(),
                    command: Some("npm install -g @agentclientprotocol/codex-acp".into()), documentation: "https://github.com/agentclientprotocol/codex-acp".into() },
                SetupAction { description: "Sign in using Codex CLI or a supported adapter authentication method.".into(),
                    command: Some("codex login".into()), documentation: "https://developers.openai.com/codex/auth".into() },
            ] }
    }

    pub fn claude() -> Self {
        Self { id: "claude".into(), name: "Claude Agent".into(), driver: ACP_STDIO_DRIVER.into(), command: "claude-agent-acp".into(),
            args: vec![], standalone_cli: Some("claude".into()), adapter_required: true, minimum_node_major: Some(22),
            setup: vec![
                SetupAction { description: "Install the Claude ACP adapter, which uses the official Claude Agent SDK.".into(),
                    command: Some("npm install -g @agentclientprotocol/claude-agent-acp".into()), documentation: "https://github.com/agentclientprotocol/claude-agent-acp".into() },
                SetupAction { description: "Authenticate Claude Code, or configure a provider-supported credential locally.".into(),
                    command: Some("claude auth login".into()), documentation: "https://code.claude.com/docs/en/authentication".into() },
            ] }
    }

    pub fn profile(self, scope: impl Into<String>) -> ProviderProfile {
        ProviderProfile {
            definition: self,
            scope: scope.into(),
            target: LaunchTarget::Discover,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub(super) enum LaunchTarget {
    Discover,
    Executable(PathBuf),
    NodeScript(PathBuf),
}

/// A profile holds private launch overrides. It intentionally does not implement
/// Debug or serialization, which could expose environment values or arguments.
#[derive(Clone)]
pub struct ProviderProfile {
    pub(super) definition: ProviderDefinition,
    pub(super) scope: String,
    pub(super) target: LaunchTarget,
    pub(super) env: BTreeMap<String, String>,
}

impl ProviderProfile {
    /// Override the default executable using an absolute path. No fallback occurs.
    #[must_use]
    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.target = LaunchTarget::Executable(path.into());
        self
    }
    /// Launch an adapter's JavaScript entry point with a discovered Node executable.
    #[must_use]
    pub fn node_script(mut self, path: impl Into<PathBuf>) -> Self {
        self.target = LaunchTarget::NodeScript(path.into());
        self
    }
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
    #[must_use]
    pub fn arg(mut self, argument: impl Into<String>) -> Self {
        self.definition.args.push(argument.into());
        self
    }
    pub fn definition(&self) -> &ProviderDefinition {
        &self.definition
    }
    pub fn inspect(&self, search: &ExecutableSearch) -> Inspection {
        discovery::inspect(self, search)
    }
}

/// Structured launch data for driver implementations. Never shell-parse arguments.
#[derive(Clone)]
pub struct ProviderLaunch {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub scope: String,
}
