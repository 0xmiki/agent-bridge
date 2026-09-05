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
use agent_bridge::acp::{AcpEvent, AcpRun, AcpSession};
use agent_bridge::acp::{RecordActors, RecordedRun, RecordingError};
use agent_bridge::records::{
    ContinuationState, ContinuationStore, MemoryStore, MessageKind, Payload, PermissionOutcome,
    RecordState, RecordStore, StoreError,
};
use agent_bridge::{ActorId, ConfigValue, Content, ContinuationId};
use agent_bridge::{RunId, RunStatus, SessionId, SlotId};
use agent_client_protocol::schema::v1::{
    ContentBlock, McpServer, McpServerHttp, McpServerStdio, SessionUpdate, StopReason,
};
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

async fn new_session(connection: &AcpConnection) -> AcpSession<'_> {
    connection
        .new_session(
            SessionId::new("app-session").unwrap(),
            SlotId::new("fixture-slot").unwrap(),
            std::env::current_dir().unwrap(),
            vec![],
        )
        .await
        .unwrap()
}

async fn next(run: &mut AcpRun<'_, '_>) -> Option<AcpEvent> {
    timeout(Duration::from_secs(3), run.next())
        .await
        .expect("prompt should make progress")
        .unwrap()
}

async fn drain(run: &mut AcpRun<'_, '_>) -> Vec<AcpEvent> {
    let mut events = Vec::new();
    while let Some(event) = next(run).await {
        events.push(event);
    }
    events
}

fn text(events: &[AcpEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)) => match &chunk.content {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn streams_ordered_text_and_continues_the_same_native_session() {
    let files = TestFiles::new();
    let log = files.path("messages");
    let connection =
        AcpConnection::connect(launch("chat").env("BRIDGE_TEST_MESSAGES", log.to_string_lossy()))
            .await
            .unwrap();
    {
        let mut session = new_session(&connection).await;
        for id in ["first", "second"] {
            let mut run = session.start_run(RunId::new(id).unwrap(), "Hello").unwrap();
            assert_eq!(run.input(), "Hello");
            assert_eq!(run.run().status(), RunStatus::Starting);
            let events = drain(&mut run).await;
            assert_eq!(text(&events), "Hello world");
            assert!(matches!(
                events.last(),
                Some(AcpEvent::Finished(StopReason::EndTurn))
            ));
            assert_eq!(run.run().status(), RunStatus::Completed);
            assert_eq!(run.run().spec().id.as_str(), id);
        }
    }
    connection.shutdown().await.unwrap();
    let requests: Vec<serde_json::Value> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "session/new")
            .count(),
        1
    );
    let prompts: Vec<_> = requests
        .iter()
        .filter(|r| r["method"] == "session/prompt")
        .collect();
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        prompts[0]["params"]["sessionId"],
        prompts[1]["params"]["sessionId"]
    );
}

#[tokio::test]
async fn routes_concurrent_native_sessions_independently() {
    let connection = AcpConnection::connect(launch("chat")).await.unwrap();
    {
        let mut first = new_session(&connection).await;
        let mut second = new_session(&connection).await;
        assert_ne!(first.info().session_id, second.info().session_id);
        let mut a = first.start_run(RunId::new("a").unwrap(), "First").unwrap();
        let mut b = second
            .start_run(RunId::new("b").unwrap(), "Second")
            .unwrap();
        let (a_events, b_events) = tokio::join!(drain(&mut a), drain(&mut b));
        assert_eq!(text(&a_events), "Hello world");
        assert_eq!(text(&b_events), "Hello world");
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn permissions_require_an_offered_option_and_cannot_be_reused() {
    let connection = AcpConnection::connect(launch("permission")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut previous_id = None;
        for run_id in ["first", "second"] {
            let mut run = session
                .start_run(RunId::new(run_id).unwrap(), "Read")
                .unwrap();
            let id = loop {
                if let Some(AcpEvent::Permission { id, request }) = next(&mut run).await {
                    assert_eq!(request.options.len(), 2);
                    break id;
                }
            };
            if let Some(previous) = previous_id {
                assert!(matches!(
                    run.respond(previous, Some("allow")),
                    Err(AcpError::InvalidPermission)
                ));
            }
            assert!(matches!(
                run.respond(id.clone(), Some("invented")),
                Err(AcpError::InvalidPermission)
            ));
            // No response is emitted until the caller chooses a valid option.
            assert!(
                timeout(Duration::from_millis(30), run.next())
                    .await
                    .is_err()
            );
            run.respond(id.clone(), Some("allow")).unwrap();
            assert!(matches!(
                run.respond(id.clone(), Some("allow")),
                Err(AcpError::InvalidPermission)
            ));
            previous_id = Some(id);
            let events = drain(&mut run).await;
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, AcpEvent::Update(SessionUpdate::ToolCallUpdate(_))))
            );
            assert_eq!(run.run().status(), RunStatus::Completed);
        }
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_dismisses_all_pending_permissions_and_waits_for_confirmation() {
    let files = TestFiles::new();
    let log = files.path("messages");
    let connection = AcpConnection::connect(
        launch("permissions").env("BRIDGE_TEST_MESSAGES", log.to_string_lossy()),
    )
    .await
    .unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_run(RunId::new("cancel-permissions").unwrap(), "Read")
            .unwrap();
        let mut ids = Vec::new();
        while ids.len() < 2 {
            if let Some(AcpEvent::Permission { id, .. }) = next(&mut run).await {
                ids.push(id);
            }
        }
        run.cancel().unwrap();
        assert_eq!(run.run().status(), RunStatus::Cancelling);
        for id in ids {
            assert!(matches!(
                run.respond(id, Some("allow")),
                Err(AcpError::InvalidPermission)
            ));
        }
        let events = drain(&mut run).await;
        assert!(matches!(
            events.last(),
            Some(AcpEvent::Finished(StopReason::Cancelled))
        ));
        assert_eq!(run.run().status(), RunStatus::Cancelled);
    }
    connection.shutdown().await.unwrap();
    let requests: Vec<serde_json::Value> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["result"]["outcome"]["outcome"] == "cancelled")
            .count(),
        2
    );
    assert!(requests.iter().any(|r| r["method"] == "session/cancel"));
}

#[tokio::test]
async fn accepts_late_updates_during_cancellation() {
    let connection = AcpConnection::connect(launch("cancel")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_run(RunId::new("cancel").unwrap(), "Hello")
            .unwrap();
        assert!(matches!(next(&mut run).await, Some(AcpEvent::Update(_))));
        run.cancel().unwrap();
        let events = drain(&mut run).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AcpEvent::Update(SessionUpdate::ToolCallUpdate(_))))
        );
        assert_eq!(run.run().status(), RunStatus::Cancelled);
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_provider_crash_leaves_the_run_unknown() {
    let connection = AcpConnection::connect(launch("prompt-crash"))
        .await
        .unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_run(RunId::new("crash").unwrap(), "Hello")
            .unwrap();
        let result = timeout(Duration::from_secs(3), run.next()).await.unwrap();
        assert!(matches!(result, Err(AcpError::Closed)));
        assert_eq!(run.run().status(), RunStatus::Unknown);
    }
    assert!(connection.shutdown().await.is_err());
}

#[tokio::test]
async fn peer_prompt_errors_fail_the_run() {
    let connection = AcpConnection::connect(launch("prompt-error"))
        .await
        .unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_run(RunId::new("error").unwrap(), "Hello")
            .unwrap();
        let result = timeout(Duration::from_secs(3), run.next()).await.unwrap();
        assert!(matches!(result, Err(AcpError::Protocol(_))));
        assert_eq!(run.run().status(), RunStatus::Failed);
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropping_a_run_retires_the_session_instead_of_mixing_late_events() {
    let connection = AcpConnection::connect(launch("cancel")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        {
            let mut run = session
                .start_run(RunId::new("abandon").unwrap(), "Hello")
                .unwrap();
            let _ = next(&mut run).await;
        }
        assert!(matches!(
            session.start_run(RunId::new("retry").unwrap(), "Hello"),
            Err(AcpError::SessionUnavailable)
        ));
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_slow_consumer_gets_an_explicit_overflow_error() {
    let connection = AcpConnection::connect(launch("flood")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_run(RunId::new("overflow").unwrap(), "Hello")
            .unwrap();
        timeout(Duration::from_secs(3), async {
            while !connection.is_closed() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(run.next().await, Err(AcpError::EventBufferFull)));
        assert_eq!(run.run().status(), RunStatus::Unknown);
    }
    assert!(connection.shutdown().await.is_err());
}

#[tokio::test]
async fn sends_workspace_and_mcp_configuration_without_modifying_it() {
    let files = TestFiles::new();
    let log = files.path("messages");
    let connection =
        AcpConnection::connect(launch("chat").env("BRIDGE_TEST_MESSAGES", log.to_string_lossy()))
            .await
            .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let server =
        McpServer::Stdio(McpServerStdio::new("app-tools", fixture()).args(vec!["tools".into()]));
    let session = connection
        .new_session(
            SessionId::new("app").unwrap(),
            SlotId::new("slot").unwrap(),
            cwd.clone(),
            vec![server],
        )
        .await
        .unwrap();
    drop(session);
    connection.shutdown().await.unwrap();
    let request: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(request["params"]["cwd"], cwd.to_string_lossy().as_ref());
    assert_eq!(request["params"]["mcpServers"][0]["name"], "app-tools");
    assert_eq!(request["params"]["mcpServers"][0]["args"][0], "tools");
}

#[tokio::test]
async fn rejects_invalid_workspace_and_unsupported_mcp_before_dispatch() {
    let connection = AcpConnection::connect(launch("chat")).await.unwrap();
    let id = SessionId::new("app").unwrap();
    let slot = SlotId::new("slot").unwrap();
    assert!(matches!(
        connection
            .new_session(id.clone(), slot.clone(), "relative", vec![])
            .await,
        Err(AcpError::InvalidWorkingDirectory)
    ));
    assert!(matches!(
        connection
            .new_session(
                id.clone(),
                slot.clone(),
                std::env::current_dir().unwrap(),
                vec![McpServer::Stdio(McpServerStdio::new(
                    "tools",
                    "relative-command"
                ))],
            )
            .await,
        Err(AcpError::InvalidMcpCommand)
    ));
    assert!(matches!(
        connection
            .new_session(
                id,
                slot,
                std::env::current_dir().unwrap(),
                vec![McpServer::Http(McpServerHttp::new(
                    "tools",
                    "http://localhost:1"
                ))]
            )
            .await,
        Err(AcpError::UnsupportedMcpTransport)
    ));
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_blank_prompt_does_not_poison_the_session() {
    let connection = AcpConnection::connect(launch("chat")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        assert!(matches!(
            session.start_run(RunId::new("blank").unwrap(), " \n"),
            Err(AcpError::EmptyPrompt)
        ));
        let mut run = session
            .start_run(RunId::new("valid").unwrap(), "Hello")
            .unwrap();
        assert_eq!(text(&drain(&mut run).await), "Hello world");
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_creation_errors_and_duplicate_ids_do_not_create_usable_handles() {
    let connection = AcpConnection::connect(launch("new-error")).await.unwrap();
    assert!(matches!(
        connection
            .new_session(
                SessionId::new("app").unwrap(),
                SlotId::new("slot").unwrap(),
                std::env::current_dir().unwrap(),
                vec![]
            )
            .await,
        Err(AcpError::Protocol(_))
    ));
    connection.shutdown().await.unwrap();
    let connection = AcpConnection::connect(launch("duplicate")).await.unwrap();
    let first = new_session(&connection).await;
    assert!(matches!(
        connection
            .new_session(
                SessionId::new("other").unwrap(),
                SlotId::new("slot").unwrap(),
                std::env::current_dir().unwrap(),
                vec![]
            )
            .await,
        Err(AcpError::SessionUnavailable)
    ));
    drop(first);
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn new_session_timeout_is_configurable_and_does_not_retry() {
    let files = TestFiles::new();
    let log = files.path("messages");
    let connection = AcpConnection::connect(
        launch("new-hang")
            .session_timeout(Duration::from_millis(100))
            .env("BRIDGE_TEST_MESSAGES", log.to_string_lossy()),
    )
    .await
    .unwrap();
    assert!(matches!(
        timeout(
            Duration::from_secs(1),
            connection.new_session(
                SessionId::new("timed-out").unwrap(),
                SlotId::new("slot").unwrap(),
                std::env::current_dir().unwrap(),
                vec![]
            )
        )
        .await
        .unwrap(),
        Err(AcpError::RequestTimedOut)
    ));
    connection.shutdown().await.unwrap();
    let messages = std::fs::read_to_string(log).unwrap();
    assert_eq!(
        messages
            .lines()
            .filter(|line| line.contains("\"method\":\"session/new\""))
            .count(),
        1
    );
}

fn actors() -> RecordActors {
    RecordActors {
        user: ActorId::new("person").unwrap(),
        agent: ActorId::new("reviewer").unwrap(),
        host: ActorId::new("app").unwrap(),
    }
}

async fn recorded_next<S: RecordStore>(run: &mut RecordedRun<'_, '_, '_, S>) -> Option<AcpEvent> {
    timeout(Duration::from_secs(3), run.next())
        .await
        .unwrap()
        .unwrap()
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn a_recorded_run_survives_sqlite_reopen_and_cannot_be_reexecuted_with_the_same_id() {
    use agent_bridge::records::SqliteStore;
    let files = TestFiles::new();
    let path = files.path("history.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    let connection = AcpConnection::connect(launch("chat")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_recorded_run(
                RunId::new("persisted-run").unwrap(),
                "Hello",
                &store,
                actors(),
            )
            .unwrap();
        while recorded_next(&mut run).await.is_some() {}
    }
    connection.shutdown().await.unwrap();
    let before = store
        .list(&SessionId::new("app-session").unwrap(), None, 100)
        .unwrap();
    drop(store);

    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .list(&SessionId::new("app-session").unwrap(), None, 100)
            .unwrap(),
        before
    );
    let connection = AcpConnection::connect(launch("chat")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        assert!(matches!(
            session.start_recorded_run(
                RunId::new("persisted-run").unwrap(),
                "Hello",
                &reopened,
                actors()
            ),
            Err(RecordingError::RunAlreadyRecorded)
        ));
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn recorded_text_has_stable_identity_without_a_write_per_delta() {
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch("chat")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_recorded_run(RunId::new("recorded").unwrap(), "Hello", &store, actors())
            .unwrap();
        assert!(matches!(
            recorded_next(&mut run).await,
            Some(AcpEvent::Update(_))
        ));
        let live = run.snapshot();
        let message = live
            .iter()
            .find(|s| {
                matches!(
                    s.record.payload,
                    Payload::Message {
                        kind: MessageKind::Agent,
                        ..
                    }
                )
            })
            .unwrap();
        let id = message.record.id.clone();
        assert!(
            matches!(&message.record.payload, Payload::Message { message, .. } if message.content == vec![Content::Text("Hello ".into())])
        );
        let before = store.get(&id).unwrap();
        assert_eq!(before.revision, 0);
        assert!(
            matches!(&before.record.payload, Payload::Message { message, .. } if message.content == vec![Content::Text(String::new())])
        );
        run.checkpoint().unwrap();
        assert_eq!(store.get(&id).unwrap().revision, 1);
        while recorded_next(&mut run).await.is_some() {}
        let saved = store.get(&id).unwrap();
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.state, RecordState::Complete);
        assert_eq!(saved.record.actor.as_str(), "reviewer");
        assert!(
            matches!(&saved.record.payload, Payload::Message { message, .. } if message.content == vec![Content::Text("Hello world".into())])
        );
    }
    connection.shutdown().await.unwrap();
    let history = store
        .list(&SessionId::new("app-session").unwrap(), None, 100)
        .unwrap();
    assert_eq!(history.len(), 3); // user, agent, stop reason
    assert!(history.iter().all(|record| record.state.is_final()));
    let run = store.get_run(&RunId::new("recorded").unwrap()).unwrap();
    assert_eq!(run.slot_id.as_str(), "fixture-slot");
    assert!(history.iter().any(|r| matches!(
        r.record.payload,
        Payload::RunFinished {
            reason: agent_bridge::records::CompletionReason::Completed
        }
    )));
}

#[tokio::test]
async fn automatic_permission_cancellation_is_recorded_with_its_request() {
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch("permissions")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_recorded_run(
                RunId::new("recorded-cancel").unwrap(),
                "Read",
                &store,
                actors(),
            )
            .unwrap();
        let mut pending = 0;
        while pending < 2 {
            if let Some(AcpEvent::Permission { .. }) = recorded_next(&mut run).await {
                pending += 1;
            }
        }
        run.cancel().unwrap();
        while recorded_next(&mut run).await.is_some() {}
    }
    connection.shutdown().await.unwrap();
    let history = store
        .list(&SessionId::new("app-session").unwrap(), None, 100)
        .unwrap();
    let responses: Vec<_> = history
        .iter()
        .filter(|s| matches!(s.record.payload, Payload::Decision { .. }))
        .collect();
    assert_eq!(responses.len(), 2);
    for response in responses {
        assert!(matches!(
            &response.record.payload,
            Payload::Decision {
                outcome: PermissionOutcome::Cancelled,
                ..
            }
        ));
        assert_eq!(response.record.actor.as_str(), "app");
        let request = store
            .get(response.record.reply_to_id.as_ref().unwrap())
            .unwrap();
        assert!(matches!(request.record.payload, Payload::Permission { .. }));
        assert_eq!(request.state, RecordState::Complete);
    }
}

#[tokio::test]
async fn recorded_permission_selection_and_tool_result_are_preserved() {
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch("permission")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_recorded_run(
                RunId::new("recorded-allow").unwrap(),
                "Read",
                &store,
                actors(),
            )
            .unwrap();
        while let Some(event) = recorded_next(&mut run).await {
            if let AcpEvent::Permission { id, .. } = event {
                // The request exists before the application can act on it.
                assert!(
                    store
                        .list(&SessionId::new("app-session").unwrap(), None, 100)
                        .unwrap()
                        .iter()
                        .any(|r| matches!(r.record.payload, Payload::Permission { .. }))
                );
                run.respond(id, Some("allow")).unwrap();
            }
        }
    }
    connection.shutdown().await.unwrap();
    let history = store
        .list(&SessionId::new("app-session").unwrap(), None, 100)
        .unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|s| matches!(s.record.payload, Payload::Tool(_)))
            .count(),
        1
    );
    assert!(history.iter().any(|s| matches!(&s.record.payload, Payload::Decision { outcome: PermissionOutcome::Selected(id), .. } if id == "allow")));
}

#[tokio::test]
async fn abandoned_recording_checkpoints_partial_text_as_interrupted() {
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch("cancel")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_recorded_run(
                RunId::new("abandoned-recording").unwrap(),
                "Hello",
                &store,
                actors(),
            )
            .unwrap();
        let _ = recorded_next(&mut run).await;
    }
    connection.shutdown().await.unwrap();
    let history = store
        .list(&SessionId::new("app-session").unwrap(), None, 100)
        .unwrap();
    let output = history
        .iter()
        .find(|s| {
            matches!(
                s.record.payload,
                Payload::Message {
                    kind: MessageKind::Agent,
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(output.state, RecordState::Interrupted);
    assert!(
        matches!(&output.record.payload, Payload::Message { message, .. } if message.content == vec![Content::Text("Hello ".into())])
    );
    assert!(
        history
            .iter()
            .any(|s| matches!(s.record.payload, Payload::Failure { .. }))
    );
}

#[tokio::test]
async fn reused_execution_identity_is_rejected_before_another_prompt_is_sent() {
    let files = TestFiles::new();
    let log = files.path("messages");
    let store = MemoryStore::default();
    let connection =
        AcpConnection::connect(launch("chat").env("BRIDGE_TEST_MESSAGES", log.to_string_lossy()))
            .await
            .unwrap();
    {
        let mut session = new_session(&connection).await;
        {
            let mut run = session
                .start_recorded_run(RunId::new("same").unwrap(), "Hello", &store, actors())
                .unwrap();
            while recorded_next(&mut run).await.is_some() {}
        }
        assert!(matches!(
            session.start_recorded_run(RunId::new("same").unwrap(), "Hello", &store, actors()),
            Err(RecordingError::RunAlreadyRecorded)
        ));
    }
    connection.shutdown().await.unwrap();
    let messages = std::fs::read_to_string(log).unwrap();
    assert_eq!(
        messages
            .lines()
            .filter(|line| line.contains("session/prompt"))
            .count(),
        1
    );
}

#[tokio::test]
async fn checkpoint_conflicts_are_reported_and_stop_recording() {
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch("cancel")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let mut run = session
            .start_recorded_run(RunId::new("conflict").unwrap(), "Hello", &store, actors())
            .unwrap();
        let _ = recorded_next(&mut run).await;
        let live = run.snapshot();
        let original = live
            .iter()
            .find(|s| {
                matches!(
                    s.record.payload,
                    Payload::Message {
                        kind: MessageKind::Agent,
                        ..
                    }
                )
            })
            .unwrap();
        let mut other = original.record.payload.clone();
        if let Payload::Message { message, .. } = &mut other {
            message.content = vec![Content::Text("another writer".into())];
        }
        store
            .checkpoint(&original.record.id, 0, other.clone(), RecordState::Open)
            .unwrap();
        assert!(matches!(
            run.checkpoint(),
            Err(RecordingError::Store(StoreError::RevisionConflict))
        ));
        assert_eq!(run.run().status(), RunStatus::Cancelling);
        assert!(matches!(run.next().await, Err(RecordingError::Closed)));
        assert_eq!(
            store.get(&original.record.id).unwrap().record.payload,
            other
        );
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn hands_off_and_resumes_the_same_native_session_across_connections() {
    let files = TestFiles::new();
    let resumed_log = files.path("resumed-messages");
    let store = MemoryStore::default();
    let first_id = ContinuationId::new("handoff-1").unwrap();

    let connection = AcpConnection::connect(launch("chat").continuation_scope("profile-a"))
        .await
        .unwrap();
    let native_id;
    {
        let mut session = new_session(&connection).await;
        native_id = session.info().session_id.clone();
        {
            let mut run = session
                .start_run(RunId::new("before-handoff").unwrap(), "Hello")
                .unwrap();
            while next(&mut run).await.is_some() {}
        }
        let saved = session.handoff(first_id.clone(), &store).unwrap();
        assert_eq!(saved.state, ContinuationState::Available);
        assert!(saved.latest);
        assert_eq!(saved.continuation.native_key, native_id.to_string());
    }
    connection.shutdown().await.unwrap();

    let connection = AcpConnection::connect(
        launch("chat")
            .continuation_scope("profile-a")
            .env("BRIDGE_TEST_MESSAGES", resumed_log.to_string_lossy()),
    )
    .await
    .unwrap();
    {
        let mut session = connection
            .resume_saved(&store, &first_id, vec![])
            .await
            .unwrap();
        assert_eq!(session.info().session_id, native_id);
        let mut run = session
            .start_run(RunId::new("after-resume").unwrap(), "Continue")
            .unwrap();
        assert_eq!(text(&drain(&mut run).await), "Hello world");
        drop(run);

        let second_id = ContinuationId::new("handoff-2").unwrap();
        let saved = session.handoff(second_id.clone(), &store).unwrap();
        assert_eq!(saved.continuation.predecessor, Some(first_id.clone()));
        assert_eq!(saved.continuation.session_id.as_str(), "app-session");
        assert_eq!(saved.continuation.slot_id.as_str(), "fixture-slot");
        assert_eq!(saved.state, ContinuationState::Available);
        assert!(!store.get_continuation(&first_id).unwrap().latest);
    }
    connection.shutdown().await.unwrap();

    let messages: Vec<serde_json::Value> = std::fs::read_to_string(resumed_log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "session/new")
            .count(),
        0
    );
    let resume = messages
        .iter()
        .find(|message| message["method"] == "session/resume")
        .unwrap();
    assert_eq!(resume["params"]["sessionId"], native_id.to_string());
    let prompt = messages
        .iter()
        .find(|message| message["method"] == "session/prompt")
        .unwrap();
    assert_eq!(prompt["params"]["sessionId"], native_id.to_string());
}

#[tokio::test]
async fn incompatible_or_unsupported_resume_does_not_claim_the_handle() {
    let store = MemoryStore::default();
    let id = ContinuationId::new("compatible").unwrap();
    let connection = AcpConnection::connect(launch("chat").continuation_scope("profile-a"))
        .await
        .unwrap();
    new_session(&connection)
        .await
        .handoff(id.clone(), &store)
        .unwrap();
    connection.shutdown().await.unwrap();

    let wrong_scope = AcpConnection::connect(launch("chat").continuation_scope("profile-b"))
        .await
        .unwrap();
    assert!(matches!(
        wrong_scope.resume_saved(&store, &id, vec![]).await,
        Err(AcpError::IncompatibleContinuation)
    ));
    wrong_scope.shutdown().await.unwrap();

    let wrong_version = AcpConnection::connect(
        launch("chat")
            .continuation_scope("profile-a")
            .env("BRIDGE_TEST_AGENT_VERSION", "2"),
    )
    .await
    .unwrap();
    assert!(matches!(
        wrong_version.resume_saved(&store, &id, vec![]).await,
        Err(AcpError::IncompatibleContinuation)
    ));
    wrong_version.shutdown().await.unwrap();

    let unsupported =
        AcpConnection::connect(launch("resume-unsupported").continuation_scope("profile-a"))
            .await
            .unwrap();
    assert!(matches!(
        unsupported.resume_saved(&store, &id, vec![]).await,
        Err(AcpError::ResumeUnsupported)
    ));
    unsupported.shutdown().await.unwrap();
    assert_eq!(
        store.get_continuation(&id).unwrap().state,
        ContinuationState::Available
    );
}

#[tokio::test]
async fn handoff_requires_a_scope_and_native_resume_support() {
    let store = MemoryStore::default();
    let unscoped = AcpConnection::connect(launch("chat")).await.unwrap();
    assert!(matches!(
        new_session(&unscoped)
            .await
            .handoff(ContinuationId::new("unscoped").unwrap(), &store),
        Err(AcpError::ContinuationScopeRequired)
    ));
    unscoped.shutdown().await.unwrap();

    let unsupported =
        AcpConnection::connect(launch("resume-unsupported").continuation_scope("profile-a"))
            .await
            .unwrap();
    assert!(matches!(
        new_session(&unsupported)
            .await
            .handoff(ContinuationId::new("unsupported").unwrap(), &store),
        Err(AcpError::ResumeUnsupported)
    ));
    unsupported.shutdown().await.unwrap();
    assert!(matches!(
        store.get_continuation(&ContinuationId::new("unscoped").unwrap()),
        Err(StoreError::MissingContinuation)
    ));
    assert!(matches!(
        store.get_continuation(&ContinuationId::new("unsupported").unwrap()),
        Err(StoreError::MissingContinuation)
    ));
}

#[tokio::test]
async fn resume_failure_after_dispatch_consumes_the_handle() {
    let store = MemoryStore::default();
    let id = ContinuationId::new("missing-native").unwrap();
    let source = AcpConnection::connect(launch("chat").continuation_scope("profile-a"))
        .await
        .unwrap();
    new_session(&source)
        .await
        .handoff(id.clone(), &store)
        .unwrap();
    source.shutdown().await.unwrap();

    let connection =
        AcpConnection::connect(launch("resume-missing").continuation_scope("profile-a"))
            .await
            .unwrap();
    assert!(matches!(
        connection.resume_saved(&store, &id, vec![]).await,
        Err(AcpError::Protocol(_))
    ));
    assert_eq!(
        store.get_continuation(&id).unwrap().state,
        ContinuationState::Claimed
    );
    assert!(matches!(
        connection.resume_saved(&store, &id, vec![]).await,
        Err(AcpError::SessionUnavailable)
    ));
    connection.shutdown().await.unwrap();

    let fresh = AcpConnection::connect(launch("resume-missing").continuation_scope("profile-a"))
        .await
        .unwrap();
    assert!(matches!(
        fresh.resume_saved(&store, &id, vec![]).await,
        Err(AcpError::Store(StoreError::ContinuationClaimed))
    ));
    fresh.shutdown().await.unwrap();
}

#[tokio::test]
async fn resume_timeout_consumes_the_handle_and_returns_promptly() {
    let store = MemoryStore::default();
    let id = ContinuationId::new("timed-out-native").unwrap();
    let source = AcpConnection::connect(launch("chat").continuation_scope("profile-a"))
        .await
        .unwrap();
    new_session(&source)
        .await
        .handoff(id.clone(), &store)
        .unwrap();
    source.shutdown().await.unwrap();

    let connection = AcpConnection::connect(
        launch("resume-hang")
            .continuation_scope("profile-a")
            .session_timeout(Duration::from_millis(100)),
    )
    .await
    .unwrap();
    assert!(matches!(
        timeout(
            Duration::from_secs(1),
            connection.resume_saved(&store, &id, vec![])
        )
        .await
        .unwrap(),
        Err(AcpError::RequestTimedOut)
    ));
    assert_eq!(
        store.get_continuation(&id).unwrap().state,
        ContinuationState::Claimed
    );
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn preflight_errors_leave_a_continuation_available() {
    let store = MemoryStore::default();
    let id = ContinuationId::new("preflight").unwrap();
    let source = AcpConnection::connect(launch("chat").continuation_scope("profile-a"))
        .await
        .unwrap();
    new_session(&source)
        .await
        .handoff(id.clone(), &store)
        .unwrap();
    source.shutdown().await.unwrap();

    let connection = AcpConnection::connect(launch("chat").continuation_scope("profile-a"))
        .await
        .unwrap();
    assert!(matches!(
        connection
            .resume_saved(
                &store,
                &id,
                vec![McpServer::Stdio(McpServerStdio::new(
                    "bad",
                    "relative-command"
                ))]
            )
            .await,
        Err(AcpError::InvalidMcpCommand)
    ));
    assert_eq!(
        store.get_continuation(&id).unwrap().state,
        ContinuationState::Available
    );
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn abandoned_or_failed_runs_cannot_be_handed_off() {
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch("cancel").continuation_scope("profile-a"))
        .await
        .unwrap();
    let mut session = new_session(&connection).await;
    {
        let mut run = session
            .start_run(RunId::new("unfinished").unwrap(), "Hello")
            .unwrap();
        let _ = next(&mut run).await;
    }
    assert!(matches!(
        session.handoff(ContinuationId::new("unsafe").unwrap(), &store),
        Err(AcpError::UnsafeHandoff)
    ));
    connection.shutdown().await.unwrap();
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn resumes_an_acp_handoff_after_the_sqlite_store_is_reopened() {
    use agent_bridge::records::SqliteStore;

    let files = TestFiles::new();
    let database = files.path("continuation.sqlite3");
    let continuation_id = ContinuationId::new("sqlite-handoff").unwrap();
    let store = SqliteStore::open(&database).unwrap();
    let source = AcpConnection::connect(launch("chat").continuation_scope("profile-a"))
        .await
        .unwrap();
    let native_id = {
        let session = new_session(&source).await;
        let native_id = session.info().session_id.clone();
        session.handoff(continuation_id.clone(), &store).unwrap();
        native_id
    };
    source.shutdown().await.unwrap();
    drop(store);

    let reopened = SqliteStore::open(&database).unwrap();
    let connection = AcpConnection::connect(launch("chat").continuation_scope("profile-a"))
        .await
        .unwrap();
    {
        let mut session = connection
            .resume_saved(&reopened, &continuation_id, vec![])
            .await
            .unwrap();
        assert_eq!(session.info().session_id, native_id);
        let mut run = session
            .start_recorded_run(
                RunId::new("after-sqlite-reopen").unwrap(),
                "Continue",
                &reopened,
                actors(),
            )
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = recorded_next(&mut run).await {
            events.push(event);
        }
        assert_eq!(text(&events), "Hello world");
        assert_eq!(run.run().spec().continuation, Some(continuation_id.clone()));
    }
    connection.shutdown().await.unwrap();
    assert_eq!(
        reopened
            .get_run(&RunId::new("after-sqlite-reopen").unwrap())
            .unwrap()
            .continuation,
        Some(continuation_id)
    );
}

#[tokio::test]
async fn configuration_discovery_and_invalid_values_do_not_dispatch_changes() {
    let files = TestFiles::new();
    let log = files.path("configuration");
    let connection =
        AcpConnection::connect(launch("config").env("BRIDGE_TEST_MESSAGES", log.to_string_lossy()))
            .await
            .unwrap();
    {
        let mut session = new_session(&connection).await;
        let state = session.configuration();
        let options = state.options.unwrap();
        assert_eq!(options[0].id, "model");
        assert_eq!(options[0].choices.len(), 2);
        assert_eq!(options[0].choices[0].group.as_deref(), Some("Provider"));
        assert!(state.values.requested.is_empty());
        assert_eq!(
            state.values.confirmed.unwrap()["model"],
            ConfigValue::Select("model-a".into())
        );
        assert!(matches!(
            session.set_model("not-offered").await,
            Err(AcpError::InvalidConfigurationValue)
        ));
        assert!(matches!(
            session
                .set_option("model", ConfigValue::Boolean(true))
                .await,
            Err(AcpError::InvalidConfigurationValue)
        ));
        assert!(matches!(
            session
                .set_option("missing", ConfigValue::Boolean(true))
                .await,
            Err(AcpError::UnknownConfigurationOption)
        ));
    }
    connection.shutdown().await.unwrap();
    assert!(
        !std::fs::read_to_string(log)
            .unwrap()
            .contains("session/set_config_option")
    );
}

#[tokio::test]
async fn switching_models_preserves_the_session_and_frozen_run_configuration() {
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch("config")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        let native = session.info().session_id.clone();
        session
            .set_option("effort", ConfigValue::Select("high".into()))
            .await
            .unwrap();
        {
            let mut run = session
                .start_recorded_run(RunId::new("model-a").unwrap(), "First", &store, actors())
                .unwrap();
            while recorded_next(&mut run).await.is_some() {}
        }
        session
            .set_option("toggle", ConfigValue::Boolean(true))
            .await
            .unwrap();
        let changed = session.set_model("model-b").await.unwrap();
        assert_eq!(
            changed.values.confirmed.as_ref().unwrap()["effort"],
            ConfigValue::Select("low".into())
        );
        assert_eq!(
            changed.values.requested["effort"],
            ConfigValue::Select("high".into())
        );
        assert_eq!(session.info().session_id, native);
        {
            let mut run = session
                .start_recorded_run(RunId::new("model-b").unwrap(), "Second", &store, actors())
                .unwrap();
            assert_eq!(run.run().spec().config, changed.values);
            while recorded_next(&mut run).await.is_some() {}
        }
    }
    connection.shutdown().await.unwrap();
    let a = store.get_run(&RunId::new("model-a").unwrap()).unwrap();
    let b = store.get_run(&RunId::new("model-b").unwrap()).unwrap();
    assert_eq!(a.session_id, b.session_id);
    assert_eq!(
        a.config.confirmed.unwrap()["model"],
        ConfigValue::Select("model-a".into())
    );
    assert_eq!(
        b.config.confirmed.as_ref().unwrap()["model"],
        ConfigValue::Select("model-b".into())
    );
    assert_eq!(b.config.requested["toggle"], ConfigValue::Boolean(true));
}

#[tokio::test]
async fn unreported_configuration_remains_unknown_and_cannot_be_selected() {
    let connection = AcpConnection::connect(launch("chat")).await.unwrap();
    {
        let mut session = new_session(&connection).await;
        assert!(session.configuration().options.is_none());
        assert!(matches!(
            session.set_model("model-a").await,
            Err(AcpError::ConfigurationUnsupported)
        ));
        let mut run = session
            .start_run(RunId::new("default").unwrap(), "Hello")
            .unwrap();
        assert!(run.run().spec().config.confirmed.is_none());
        while next(&mut run).await.is_some() {}
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_setter_timeout_blocks_dispatch_and_handoff_until_confirmed() {
    let connection = AcpConnection::connect(
        launch("config-hang")
            .session_timeout(Duration::from_millis(100))
            .continuation_scope("profile"),
    )
    .await
    .unwrap();
    {
        let mut session = new_session(&connection).await;
        assert!(matches!(
            session.set_model("model-b").await,
            Err(AcpError::RequestTimedOut)
        ));
        assert!(session.configuration().values.confirmed.is_none());
        assert!(matches!(
            session.start_run(RunId::new("blocked").unwrap(), "Hello"),
            Err(AcpError::ConfigurationUncertain)
        ));
        assert!(matches!(
            session.handoff(
                ContinuationId::new("blocked-handoff").unwrap(),
                &MemoryStore::default()
            ),
            Err(AcpError::ConfigurationUncertain)
        ));
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn late_configuration_acknowledgement_recovers_after_timeout() {
    let connection =
        AcpConnection::connect(launch("config-late").session_timeout(Duration::from_millis(100)))
            .await
            .unwrap();
    {
        let mut session = new_session(&connection).await;
        assert!(matches!(
            session.set_model("model-b").await,
            Err(AcpError::RequestTimedOut)
        ));
        timeout(Duration::from_secs(2), async {
            while session.configuration().pending {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!session.configuration().uncertain);
        assert_eq!(
            session.configuration().values.confirmed.unwrap()["model"],
            ConfigValue::Select("model-b".into())
        );
        let mut run = session
            .start_run(RunId::new("after-late").unwrap(), "Hello")
            .unwrap();
        while next(&mut run).await.is_some() {}
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn setter_errors_and_mismatched_reports_are_explicit() {
    for mode in ["config-error", "config-reject"] {
        let connection = AcpConnection::connect(launch(mode)).await.unwrap();
        {
            let mut session = new_session(&connection).await;
            let result = session.set_model("model-b").await;
            if mode == "config-error" {
                assert!(matches!(result, Err(AcpError::Protocol(_))));
                assert!(session.configuration().uncertain);
            } else {
                assert!(matches!(result, Err(AcpError::ConfigurationRejected)));
                assert_eq!(
                    session.configuration().values.confirmed.unwrap()["model"],
                    ConfigValue::Select("model-a".into())
                );
            }
            assert!(session.configuration().values.requested.is_empty());
        }
        connection.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn configuration_updates_immediately_after_setup_are_not_lost() {
    let connection = AcpConnection::connect(launch("config-idle")).await.unwrap();
    {
        let session = new_session(&connection).await;
        timeout(Duration::from_secs(2), async {
            while session.configuration().values.confirmed.as_ref().unwrap()["model"]
                != ConfigValue::Select("model-b".into())
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn provider_configuration_changes_are_recorded_without_rewriting_dispatch_settings() {
    let store = MemoryStore::default();
    let connection = AcpConnection::connect(launch("config-fallback"))
        .await
        .unwrap();
    {
        let mut session = new_session(&connection).await;
        {
            let mut run = session
                .start_recorded_run(RunId::new("fallback").unwrap(), "Hello", &store, actors())
                .unwrap();
            while recorded_next(&mut run).await.is_some() {}
        }
        assert_eq!(
            session.configuration().values.confirmed.unwrap()["model"],
            ConfigValue::Select("model-b".into())
        );
    }
    connection.shutdown().await.unwrap();
    assert_eq!(
        store
            .get_run(&RunId::new("fallback").unwrap())
            .unwrap()
            .config
            .confirmed
            .unwrap()["model"],
        ConfigValue::Select("model-a".into())
    );
    let history = store
        .list(&SessionId::new("app-session").unwrap(), None, 100)
        .unwrap();
    assert!(history.iter().any(|r| matches!(&r.record.payload, Payload::Extension {namespace,name,data}
        if namespace == "agent_bridge" && name == "configuration_report" && data["confirmed"]["model"]["value"] == "model-b")));
}
