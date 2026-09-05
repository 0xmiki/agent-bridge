#![cfg(feature = "acp")]

use std::{
    path::PathBuf,
    process::Command,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use agent_bridge::acp::{AcpConnection, AcpError, AcpLaunch};
use tokio::time::timeout;

fn fixture() -> PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY
        .get_or_init(|| {
            let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
            std::fs::create_dir_all(&directory).unwrap();
            let binary = directory.join(format!(
                "acp-fixture-{}{}",
                std::process::id(),
                std::env::consts::EXE_SUFFIX
            ));
            let output = Command::new("rustc")
                .arg("--edition=2024")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/support/acp_fixture.rs"
                ))
                .arg("-o")
                .arg(&binary)
                .output()
                .expect("rustc must be available to build the subprocess fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            binary
        })
        .clone()
}

fn launch(mode: &str) -> AcpLaunch {
    AcpLaunch::new(fixture())
        .arg(mode)
        .initialize_timeout(Duration::from_secs(5))
}

struct TestFiles(PathBuf);

impl TestFiles {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "acp-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn initializes_and_reports_advertised_capabilities() {
    let files = TestFiles::new();
    let connection = AcpConnection::connect(launch("normal").env(
        "BRIDGE_TEST_REQUEST",
        files.path("request.json").to_string_lossy(),
    ))
    .await
    .unwrap();
    assert_eq!(
        connection.info().agent_info.as_ref().unwrap().name,
        "fixture"
    );
    assert!(connection.info().agent_capabilities.load_session);
    assert!(
        connection
            .info()
            .agent_capabilities
            .prompt_capabilities
            .image
    );
    let request: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(files.path("request.json")).unwrap())
            .unwrap();
    assert_eq!(request["method"], "initialize");
    assert_eq!(request["params"]["protocolVersion"], 1);
    assert_eq!(request["params"]["clientInfo"]["name"], "agent-bridge");
    // Do not advertise filesystem/terminal operations that we cannot implement.
    assert_ne!(request["params"]["clientCapabilities"]["terminal"], true);
    assert_ne!(
        request["params"]["clientCapabilities"]["fs"]["readTextFile"],
        true
    );
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_executable_is_an_error() {
    let files = TestFiles::new();
    let result = AcpConnection::connect(AcpLaunch::new(files.path("missing-agent"))).await;
    assert!(matches!(result, Err(AcpError::Protocol(_))));
}

#[tokio::test]
async fn early_exit_is_reported_without_waiting_for_initialization_timeout() {
    let result = timeout(
        Duration::from_secs(3),
        AcpConnection::connect(launch("exit")),
    )
    .await
    .expect("early exit should settle promptly");
    assert!(matches!(result, Err(AcpError::Protocol(_))));
}

#[tokio::test]
async fn rejects_an_unsupported_protocol_version() {
    let result = AcpConnection::connect(launch("version")).await;
    assert!(matches!(result, Err(AcpError::UnsupportedProtocolVersion)));
}

#[tokio::test]
async fn rejects_a_malformed_initialization_response() {
    let result = AcpConnection::connect(launch("malformed")).await;
    assert!(matches!(result, Err(AcpError::Protocol(_))));
}

#[tokio::test]
async fn drains_stderr_without_blocking_the_handshake() {
    let connection = AcpConnection::connect(launch("stderr")).await.unwrap();
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn observes_failure_after_initialization() {
    let connection = AcpConnection::connect(launch("crash")).await.unwrap();
    let result = timeout(Duration::from_secs(3), connection.wait_closed())
        .await
        .unwrap();
    assert!(matches!(result, Err(AcpError::Protocol(_))));
}

#[cfg(target_os = "linux")]
async fn assert_process_stopped(pid_file: PathBuf) {
    let pid: u32 = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            // An orphan zombie has exited even if the container has not reaped it.
            if stat
                .as_ref()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                || stat.as_ref().is_ok_and(|s| {
                    s.rsplit_once(')')
                        .is_some_and(|(_, tail)| tail.trim_start().starts_with('Z'))
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned provider process should exit");
}

#[tokio::test]
async fn initialization_timeout_cleans_up_the_process() {
    let files = TestFiles::new();
    let pid_file = files.path("pid");
    let result = AcpConnection::connect(
        launch("silent")
            .env("BRIDGE_TEST_PID", pid_file.to_string_lossy())
            .initialize_timeout(Duration::from_millis(300)),
    )
    .await;
    assert!(matches!(result, Err(AcpError::InitializationTimedOut)));
    #[cfg(target_os = "linux")]
    assert_process_stopped(pid_file).await;
}

#[tokio::test]
async fn dropping_an_initialized_connection_cleans_up_the_process() {
    let files = TestFiles::new();
    let pid_file = files.path("pid");
    let connection = AcpConnection::connect(
        launch("stubborn").env("BRIDGE_TEST_PID", pid_file.to_string_lossy()),
    )
    .await
    .unwrap();
    drop(connection);
    #[cfg(target_os = "linux")]
    assert_process_stopped(pid_file).await;
}

#[tokio::test]
async fn shutdown_is_bounded_when_the_process_ignores_eof() {
    let files = TestFiles::new();
    let pid_file = files.path("pid");
    let connection = AcpConnection::connect(
        launch("stubborn").env("BRIDGE_TEST_PID", pid_file.to_string_lossy()),
    )
    .await
    .unwrap();
    timeout(Duration::from_secs(4), connection.shutdown())
        .await
        .unwrap()
        .unwrap();
    #[cfg(target_os = "linux")]
    assert_process_stopped(pid_file).await;
}

async fn wait_for_file(path: &std::path::Path) {
    timeout(Duration::from_secs(5), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture should create its file");
}

#[tokio::test]
async fn cancelling_connect_cleans_up_before_the_handshake_finishes() {
    let files = TestFiles::new();
    let pid_file = files.path("pid");
    {
        let mut pending = Box::pin(AcpConnection::connect(
            launch("silent").env("BRIDGE_TEST_PID", pid_file.to_string_lossy()),
        ));
        tokio::select! {
            _ = &mut pending => panic!("silent agent should not initialize"),
            _ = wait_for_file(&pid_file) => {},
        }
        // Dropping the pending future must abort its owned connection task.
    }
    #[cfg(target_os = "linux")]
    assert_process_stopped(pid_file).await;
}

#[tokio::test]
async fn arguments_are_passed_without_shell_interpretation() {
    let files = TestFiles::new();
    let literal = "$(not-a-command) ; argument with spaces";
    let connection = AcpConnection::connect(launch("normal").arg(literal).env(
        "BRIDGE_TEST_ARGUMENT",
        files.path("argument").to_string_lossy(),
    ))
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(files.path("argument")).unwrap(),
        literal
    );
    connection.shutdown().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dropping_a_connection_also_stops_wrapper_descendants() {
    let files = TestFiles::new();
    let child_pid = files.path("child");
    let descendant_pid = files.path("descendant");
    let connection = AcpConnection::connect(
        launch("tree")
            .env("BRIDGE_TEST_PID", child_pid.to_string_lossy())
            .env("BRIDGE_TEST_DESCENDANT", descendant_pid.to_string_lossy()),
    )
    .await
    .unwrap();
    wait_for_file(&descendant_pid).await;
    drop(connection);
    assert_process_stopped(child_pid).await;
    assert_process_stopped(descendant_pid).await;
}
