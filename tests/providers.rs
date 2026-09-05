#![cfg(feature = "providers")]
use agent_bridge::providers::{ExecutableSearch, ProviderDefinition, SetupIssue};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "providers-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn executable(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
    fn search(&self) -> ExecutableSearch {
        ExecutableSearch::from_directories([self.0.clone()])
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn distinguishes_missing_cli_from_missing_adapter() {
    let files = Files::new();
    let report = ProviderDefinition::opencode()
        .profile("default")
        .inspect(&files.search());
    assert!(!report.is_launchable());
    assert_eq!(
        report.report.issues,
        vec![SetupIssue::MissingCli("opencode".into())]
    );
    let codex = files.executable("codex", b"binary");
    let report = ProviderDefinition::codex()
        .profile("default")
        .inspect(&files.search());
    assert_eq!(report.report.standalone_cli, Some(codex));
    assert_eq!(
        report.report.issues,
        vec![SetupIssue::MissingAdapter("codex-acp".into())]
    );
}

#[test]
fn adapter_can_be_launchable_without_a_separate_cli() {
    let files = Files::new();
    let adapter = files.executable("claude-agent-acp", b"native binary");
    let report = ProviderDefinition::claude()
        .profile("default")
        .inspect(&files.search());
    assert!(report.report.standalone_cli.is_none());
    assert!(report.is_launchable());
    assert_eq!(
        report.into_resolved().ok().unwrap().launch().executable,
        adapter
    );
}

#[test]
fn node_script_requires_a_runtime_but_not_an_executable_bit() {
    let files = Files::new();
    let script = files.path("adapter.js");
    std::fs::write(&script, b"// an adapter").unwrap();
    let profile = ProviderDefinition::claude()
        .profile("default")
        .node_script(&script);
    let report = profile.inspect(&files.search());
    assert_eq!(
        report.report.issues,
        vec![SetupIssue::MissingRuntime("node".into())]
    );
    let node = files.executable("node", b"binary");
    let report = profile.inspect(&files.search());
    let launch = report.into_resolved().ok().unwrap();
    assert_eq!(launch.launch().executable, node);
    assert_eq!(launch.launch().args, vec![script.to_str().unwrap()]);
}

#[test]
fn recognizes_npm_node_entrypoints_without_executing_them() {
    let files = Files::new();
    let marker = files.path("should-not-exist");
    let script = files.executable(
        "codex-acp",
        format!(
            "#!/usr/bin/env node\nrequire('fs').writeFileSync({:?}, 'ran');",
            marker.to_string_lossy()
        )
        .as_bytes(),
    );
    let node = files.executable("node", b"binary");
    let inspection = ProviderDefinition::codex()
        .profile("default")
        .inspect(&files.search());
    let resolved = inspection.into_resolved().ok().unwrap();
    assert_eq!(resolved.launch().executable, node);
    assert_eq!(resolved.launch().args[0], script.to_string_lossy());
    assert!(!marker.exists());
}

#[test]
fn explicit_missing_paths_do_not_fall_back_to_another_installation() {
    let files = Files::new();
    files.executable("opencode", b"binary");
    let missing = files.path("missing");
    let report = ProviderDefinition::opencode()
        .profile("default")
        .executable(&missing)
        .inspect(&files.search());
    assert_eq!(report.report.issues, vec![SetupIssue::MissingFile(missing)]);
    let relative = ProviderDefinition::opencode()
        .profile("default")
        .executable("relative")
        .inspect(&files.search());
    assert_eq!(
        relative.report.issues,
        vec![SetupIssue::InvalidPath(PathBuf::from("relative"))]
    );
}

#[test]
fn preserves_literal_arguments_environment_and_unambiguous_scope() {
    let files = Files::new();
    let binary = files.executable("custom", b"binary");
    let a = ProviderDefinition::custom("a/b", "custom")
        .profile("c")
        .executable(&binary)
        .arg("$(not-a-command) ; spaces")
        .env("TEST_PROFILE", "value")
        .inspect(&files.search())
        .into_resolved()
        .ok()
        .unwrap();
    let b = ProviderDefinition::custom("a", "custom")
        .profile("b/c")
        .inspect(&files.search())
        .into_resolved()
        .ok()
        .unwrap();
    assert_eq!(a.launch().args, vec!["$(not-a-command) ; spaces"]);
    assert_eq!(a.launch().env["TEST_PROFILE"], "value");
    assert_ne!(a.launch().scope, b.launch().scope);
}

#[test]
fn rejects_command_shell_shims_and_invalid_profiles() {
    let files = Files::new();
    let shim = files.executable("adapter.cmd", b"@node adapter.js");
    let report = ProviderDefinition::claude()
        .profile("default")
        .executable(&shim)
        .inspect(&files.search());
    assert_eq!(
        report.report.issues,
        vec![SetupIssue::UnsupportedLauncher(shim)]
    );
    let report = ProviderDefinition::opencode()
        .profile(" ")
        .inspect(&files.search());
    assert_eq!(report.report.issues, vec![SetupIssue::InvalidProfile]);
}

#[cfg(unix)]
#[test]
fn explicit_non_executable_files_are_reported() {
    let files = Files::new();
    let path = files.path("adapter");
    std::fs::write(&path, b"binary").unwrap();
    let report = ProviderDefinition::codex()
        .profile("default")
        .executable(&path)
        .inspect(&files.search());
    assert_eq!(report.report.issues, vec![SetupIssue::NotExecutable(path)]);
}

#[test]
fn opencode_uses_its_native_acp_subcommand() {
    let files = Files::new();
    let path = files.executable("opencode", b"binary");
    let result = ProviderDefinition::opencode()
        .profile("default")
        .inspect(&files.search())
        .into_resolved()
        .ok()
        .unwrap();
    assert_eq!(result.launch().executable, path);
    assert_eq!(result.launch().args, vec!["acp"]);
    assert_eq!(result.definition().driver, "acp-stdio");
}

#[cfg(unix)]
#[tokio::test]
async fn an_old_node_runtime_is_rejected_before_the_adapter_is_launched() {
    use agent_bridge::providers::{AcpDriver, ProviderDriver, ProviderError};
    let files = Files::new();
    files.executable("node", b"#!/bin/sh\nprintf 'v18.0.0\\n'\n");
    let script = files.path("adapter.js");
    std::fs::write(&script, b"// not executed").unwrap();
    let resolved = ProviderDefinition::claude()
        .profile("test")
        .node_script(script)
        .inspect(&files.search())
        .into_resolved()
        .ok()
        .unwrap();
    assert!(matches!(
        AcpDriver.connect(resolved).await,
        Err(ProviderError::UnsupportedRuntime {
            minimum_major: 22,
            found_major: Some(18)
        })
    ));
}
