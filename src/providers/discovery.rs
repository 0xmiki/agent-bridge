use super::{LaunchTarget, ProviderDefinition, ProviderLaunch, ProviderProfile, SetupAction};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct ExecutableSearch {
    directories: Vec<PathBuf>,
}

impl ExecutableSearch {
    /// PATH plus common per-user CLI locations. No shell or package manager is run.
    pub fn current() -> Self {
        let mut directories: Vec<_> = env::var_os("PATH")
            .map(|path| env::split_paths(&path).collect())
            .unwrap_or_default();
        if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
            let home = PathBuf::from(home);
            for suffix in [".local/bin", ".opencode/bin", ".bun/bin", ".npm-global/bin"] {
                directories.push(home.join(suffix));
            }
        }
        Self::from_directories(directories)
    }

    /// Relative and empty PATH entries are excluded. Explicit executable paths
    /// remain available through ProviderProfile without consulting this list.
    pub fn from_directories(directories: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut result = Vec::new();
        for directory in directories {
            if directory.is_absolute() && !result.contains(&directory) {
                result.push(directory);
            }
        }
        Self {
            directories: result,
        }
    }

    fn find(&self, name: &str) -> Option<PathBuf> {
        if Path::new(name).components().count() != 1 {
            return None;
        }
        for directory in &self.directories {
            for suffix in executable_suffixes() {
                let candidate = directory.join(format!("{name}{suffix}"));
                if executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

fn executable_suffixes() -> &'static [&'static str] {
    if cfg!(windows) {
        &["", ".exe", ".com", ".cmd", ".bat"]
    } else {
        &[""]
    }
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupIssue {
    MissingCli(String),
    MissingAdapter(String),
    MissingRuntime(String),
    InvalidPath(PathBuf),
    MissingFile(PathBuf),
    NotExecutable(PathBuf),
    Unreadable(PathBuf),
    UnsupportedLauncher(PathBuf),
    InvalidProfile,
}

#[derive(Debug, Clone)]
pub struct InstallationReport {
    pub provider: String,
    pub driver: String,
    /// The independently installed CLI is informational for bundled adapters.
    pub standalone_cli: Option<PathBuf>,
    pub entrypoint: Option<PathBuf>,
    pub runtime: Option<PathBuf>,
    pub issues: Vec<SetupIssue>,
    pub setup: Vec<SetupAction>,
}

pub struct Inspection {
    pub report: InstallationReport,
    resolved: Option<ResolvedProvider>,
}
impl Inspection {
    /// Ready to attempt a connection, not proof of login or model access.
    pub fn is_launchable(&self) -> bool {
        self.resolved.is_some()
    }
    pub fn into_resolved(self) -> Result<ResolvedProvider, Box<InstallationReport>> {
        self.resolved.ok_or_else(|| Box::new(self.report))
    }
}

#[derive(Clone)]
pub struct ResolvedProvider {
    pub(super) definition: ProviderDefinition,
    pub(super) launch: ProviderLaunch,
    pub(super) node_runtime: Option<PathBuf>,
}
impl ResolvedProvider {
    pub fn definition(&self) -> &ProviderDefinition {
        &self.definition
    }
    pub fn launch(&self) -> &ProviderLaunch {
        &self.launch
    }
}

pub(super) fn inspect(profile: &ProviderProfile, search: &ExecutableSearch) -> Inspection {
    let definition = &profile.definition;
    let mut report = InstallationReport {
        provider: definition.id.clone(),
        driver: definition.driver.clone(),
        standalone_cli: definition
            .standalone_cli
            .as_deref()
            .and_then(|name| search.find(name)),
        entrypoint: None,
        runtime: None,
        issues: vec![],
        setup: definition.setup.clone(),
    };
    if definition.id.trim().is_empty()
        || definition.driver.trim().is_empty()
        || profile.scope.trim().is_empty()
    {
        report.issues.push(SetupIssue::InvalidProfile);
        return Inspection {
            report,
            resolved: None,
        };
    }
    let path = match &profile.target {
        LaunchTarget::Discover => match search.find(&definition.command) {
            Some(path) => path,
            None => {
                report.issues.push(if definition.adapter_required {
                    SetupIssue::MissingAdapter(definition.command.clone())
                } else {
                    SetupIssue::MissingCli(definition.command.clone())
                });
                return Inspection {
                    report,
                    resolved: None,
                };
            }
        },
        LaunchTarget::Executable(path) | LaunchTarget::NodeScript(path) => path.clone(),
    };
    report.entrypoint = Some(path.clone());
    if !path.is_absolute() {
        report.issues.push(SetupIssue::InvalidPath(path.clone()));
    } else if !path.is_file() {
        report.issues.push(SetupIssue::MissingFile(path.clone()));
    } else if !matches!(profile.target, LaunchTarget::NodeScript(_)) && !executable(&path) {
        report.issues.push(SetupIssue::NotExecutable(path.clone()));
    }
    if !report.issues.is_empty() {
        return Inspection {
            report,
            resolved: None,
        };
    }
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
    {
        report.issues.push(SetupIssue::UnsupportedLauncher(path));
        return Inspection {
            report,
            resolved: None,
        };
    }
    let mut prefix = [0_u8; 256];
    let bytes = match fs::File::open(&path).and_then(|mut file| file.read(&mut prefix)) {
        Ok(bytes) => bytes,
        Err(_) => {
            report.issues.push(SetupIssue::Unreadable(path));
            return Inspection {
                report,
                resolved: None,
            };
        }
    };
    let first_line = String::from_utf8_lossy(&prefix[..bytes]);
    let first_line = first_line.lines().next().unwrap_or_default().trim();
    let needs_node = matches!(profile.target, LaunchTarget::NodeScript(_))
        || matches!(
            first_line,
            "#!/usr/bin/env node" | "#!/usr/bin/node" | "#!/usr/local/bin/node"
        );
    let mut args = definition.args.clone();
    let program = if needs_node {
        let Some(node) = search.find("node") else {
            report
                .issues
                .push(SetupIssue::MissingRuntime("node".into()));
            return Inspection {
                report,
                resolved: None,
            };
        };
        let Some(script) = path.to_str() else {
            report.issues.push(SetupIssue::InvalidPath(path));
            return Inspection {
                report,
                resolved: None,
            };
        };
        args.insert(0, script.into());
        report.runtime = Some(node.clone());
        node
    } else {
        path
    };
    let resolved = ResolvedProvider {
        definition: definition.clone(),
        node_runtime: report.runtime.clone(),
        launch: ProviderLaunch {
            executable: program,
            args,
            env: profile.env.clone(),
            scope: format!(
                "provider/{}:{}/profile/{}:{}",
                definition.id.len(),
                definition.id,
                profile.scope.len(),
                profile.scope
            ),
        },
    };
    Inspection {
        report,
        resolved: Some(resolved),
    }
}
