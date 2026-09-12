//! Standalone std-only subprocess fixture, compiled by the integration tests.
use std::{
    io::{self, BufRead, Write},
    time::Duration,
};

fn main() {
    let mode = std::env::args().nth(1).expect("fixture mode");
    if mode == "host-consumer" {
        consume_host();
        return;
    }
    if let Ok(path) = std::env::var("BRIDGE_TEST_PID") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    if let Ok(path) = std::env::var("BRIDGE_TEST_ARGUMENT") {
        std::fs::write(path, std::env::args().nth(2).unwrap()).unwrap();
    }
    let _descendant = if mode == "tree" || mode.starts_with("host-tree-") {
        Some(
            std::process::Command::new(std::env::current_exe().unwrap())
                .arg("silent")
                .env(
                    "BRIDGE_TEST_PID",
                    std::env::var("BRIDGE_TEST_DESCENDANT").unwrap(),
                )
                .spawn()
                .unwrap(),
        )
    } else {
        None
    };
    if mode == "exit" {
        eprintln!("fixture refused to start");
        std::process::exit(7);
    }
    if mode == "silent" {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut request = String::new();
    input.read_line(&mut request).unwrap();
    if let Ok(path) = std::env::var("BRIDGE_TEST_REQUEST") {
        std::fs::write(path, &request).unwrap();
    }
    // The fixture only echoes the SDK-generated scalar request ID. It is not a
    // general JSON parser or an implementation used by the library.
    let id_tail = request.split_once("\"id\"").unwrap().1;
    let id = id_tail.split_once(':').unwrap().1.trim_start();
    let id = id.split([',', '}']).next().unwrap().trim();
    if mode == "stderr" {
        io::stderr().write_all(&vec![b'x'; 256 * 1024]).unwrap();
        io::stderr().flush().unwrap();
    }
    let version = if mode == "version" { 999 } else { 1 };
    let sessions = if mode == "resume-unsupported" {
        "{}"
    } else {
        r#"{"resume":{}}"#
    };
    let agent_version = std::env::var("BRIDGE_TEST_AGENT_VERSION").unwrap_or_else(|_| "1".into());
    let image_support = std::env::var_os("BRIDGE_TEST_NO_IMAGES").is_none();
    if mode == "malformed" {
        println!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":\"bad\"}}}}");
    } else {
        println!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":{version},\"agentCapabilities\":{{\"loadSession\":true,\"promptCapabilities\":{{\"image\":{image_support}}},\"sessionCapabilities\":{sessions}}},\"agentInfo\":{{\"name\":\"fixture\",\"version\":\"{agent_version}\"}},\"authMethods\":[]}}}}"
        );
    }
    io::stdout().flush().unwrap();
    if mode == "crash" {
        // Ensure initialization reaches the client before exercising later failure.
        std::thread::sleep(Duration::from_millis(100));
        std::process::exit(9);
    }
    if mode == "stubborn" || mode == "tree" {
        std::thread::sleep(Duration::from_secs(60));
    } else if [
        "chat",
        "permission",
        "permissions",
        "cancel",
        "prompt-error",
        "prompt-crash",
        "flood",
        "new-error",
        "new-hang",
        "duplicate",
        "resume-missing",
        "resume-hang",
        "resume-unsupported",
    ]
    .contains(&mode.as_str())
        || mode.starts_with("config")
        || mode.starts_with("json-")
        || mode.starts_with("host-")
    {
        serve_sessions(&mode, &mut input);
    } else {
        // Normal ACP shutdown closes stdin. No invented shutdown RPC is required.
        for line in input.lines() {
            if line.is_err() {
                break;
            }
        }
    }
}

fn scalar<'a>(message: &'a str, key: &str) -> &'a str {
    let marker = format!("\"{key}\"");
    let tail = message
        .split_once(&marker)
        .unwrap()
        .1
        .split_once(':')
        .unwrap()
        .1
        .trim_start();
    tail.split([',', '}']).next().unwrap().trim()
}

// Unlike a JS stream, this consumer does no background pipe draining after the
// first delta. It lets host tests create a genuinely blocked stdout writer.
fn consume_host() {
    use std::process::{Command, Stdio};
    let args: Vec<_> = std::env::args().collect();
    let mut host = Command::new(&args[2]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    std::fs::write(std::env::var("BRIDGE_TEST_HOST_PID").unwrap(), host.id().to_string()).unwrap();
    let mut output = io::BufReader::new(host.stdout.take().unwrap());
    let executable = std::env::current_exe().unwrap();
    let mode = std::env::var("BRIDGE_TEST_PROVIDER_MODE").unwrap();
    writeln!(host.stdin.as_mut().unwrap(), r#"{{"version":1,"id":"create","method":"create_session","params":{{"database":"{}","workspace":"{}","executable":"{}","args":["{mode}"]}}}}"#, args[3], args[4], executable.display()).unwrap();
    let mut line = String::new();
    loop {
        line.clear();
        assert!(output.read_line(&mut line).unwrap() > 0);
        if line.contains(r#""id":"create""#) { break; }
    }
    assert!(line.contains(r#""ok":true"#), "{line}");
    let session = scalar(&line, "session_id");
    writeln!(host.stdin.as_mut().unwrap(), r#"{{"version":1,"id":"run","method":"run","params":{{"session_id":{session},"prompt":"wait"}}}}"#).unwrap();
    loop {
        line.clear();
        assert!(output.read_line(&mut line).unwrap() > 0);
        if line.contains(r#""event":"text_delta""#) { break; }
    }
    std::fs::write(std::env::var("BRIDGE_TEST_CONSUMER_READY").unwrap(), "ready").unwrap();
    line.clear();
    io::stdin().read_line(&mut line).unwrap();
    if line.trim() == "eof" { drop(host.stdin.take()); }
    if line.trim() == "close" { drop(output); }
    // Child::wait closes its own stdin handle. Keep it outside Child so the
    // stalled-output case tests the writer deadline independently of EOF.
    let _keep_input_open = host.stdin.take();
    let status = host.wait().unwrap();
    std::process::exit(status.code().unwrap_or(1));
}

fn reply(id: &str, result: &str) {
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}");
    io::stdout().flush().unwrap();
}

fn update(session: &str, update: &str) {
    println!(
        "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{session},\"update\":{update}}}}}"
    );
    io::stdout().flush().unwrap();
}

fn serve_sessions(mode: &str, input: &mut impl BufRead) {
    use std::collections::HashMap;
    // Native session -> (prompt request ID, remaining decisions, any cancelled).
    let mut pending: HashMap<String, (String, usize, bool)> = HashMap::new();
    let mut permissions: HashMap<String, String> = HashMap::new();
    let mut count = 0;
    let mut settings: HashMap<String, (String, bool)> = HashMap::new();
    let mut remembered = String::new();
    let mut turn = 0;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).unwrap() == 0 {
            return;
        }
        if let Ok(path) = std::env::var("BRIDGE_TEST_MESSAGES") {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            file.write_all(line.as_bytes()).unwrap();
        }
        if line.contains("\"method\":\"session/new\"") {
            let id = scalar(&line, "id");
            if mode == "new-hang" {
                continue;
            } else if mode == "new-error" {
                println!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32000,\"message\":\"fixture session error\"}}}}"
                );
                io::stdout().flush().unwrap();
            } else {
                if mode != "duplicate" || count == 0 {
                    count += 1;
                }
                let session = format!("\"native-{count}\"");
                settings.insert(session.clone(), ("model-a".into(), false));
                if mode.starts_with("config") {
                    reply(
                        id,
                        &format!(
                            "{{\"sessionId\":{session},\"configOptions\":{}}}",
                            config_options("model-a", false)
                        ),
                    );
                    if mode == "config-idle" {
                        settings.insert(session.clone(), ("model-b".into(), false));
                        update(
                            &session,
                            &format!(
                                "{{\"sessionUpdate\":\"config_option_update\",\"configOptions\":{}}}",
                                config_options("model-b", false)
                            ),
                        );
                    }
                } else {
                    reply(id, &format!("{{\"sessionId\":{session}}}"));
                }
            }
        } else if line.contains("\"method\":\"session/resume\"") {
            let id = scalar(&line, "id");
            if mode == "resume-missing" {
                println!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32000,\"message\":\"native session missing\"}}}}"
                );
                io::stdout().flush().unwrap();
            } else if mode != "resume-hang" {
                reply(id, "{}");
            }
        } else if line.contains("\"method\":\"session/set_config_option\"") {
            if mode == "config-hang" {
                continue;
            }
            let id = scalar(&line, "id");
            if mode == "config-error" {
                println!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32000,\"message\":\"configuration failed\"}}}}"
                );
                io::stdout().flush().unwrap();
                continue;
            }
            let session = scalar(&line, "sessionId");
            let option = scalar(&line, "configId").trim_matches('"');
            let value = scalar(&line, "value").trim_matches('"');
            let (model, flag) = settings.get_mut(session).unwrap();
            if mode != "config-reject" {
                if option == "model" {
                    *model = value.into();
                }
                if option == "toggle" {
                    *flag = value == "true";
                }
            }
            if mode == "config-late" {
                std::thread::sleep(Duration::from_millis(200));
            }
            reply(
                id,
                &format!("{{\"configOptions\":{}}}", config_options(model, *flag)),
            );
        } else if line.contains("\"method\":\"session/delete\"") {
            let id = scalar(&line, "id");
            if mode == "host-delete-error" {
                println!(r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32601,"message":"deletion unsupported"}}}}"#);
                io::stdout().flush().unwrap();
            } else {
                if let Ok(path) = std::env::var("BRIDGE_TEST_DELETED") { std::fs::write(path, scalar(&line, "sessionId")).unwrap(); }
                reply(id, "{}");
            }
        } else if line.contains("\"method\":\"session/prompt\"") {
            let session = scalar(&line, "sessionId");
            let id = scalar(&line, "id");
            if mode == "host-state" {
                if turn == 0 {
                    remembered = scalar(&line, "text").trim_matches('"').to_owned();
                    assert!(remembered.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
                }
                let suffix = if turn == 0 { ":first" } else { ":recall" };
                for (chunk, text) in [remembered.as_str(), suffix].into_iter().enumerate() {
                    if let Ok(gate) = std::env::var("BRIDGE_TEST_GATE") {
                        std::fs::write(format!("{gate}.ready-{turn}-{chunk}"), "ready").unwrap();
                        let deadline = std::time::Instant::now() + Duration::from_secs(10);
                        while !std::path::Path::new(&format!("{gate}.go-{turn}-{chunk}")).exists() {
                            assert!(std::time::Instant::now() < deadline, "fixture gate timed out");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    update(session, &format!(r#"{{"sessionUpdate":"agent_message_chunk","messageId":"answer","content":{{"type":"text","text":"{text}"}}}}"#));
                }
                turn += 1;
                reply(id, r#"{"stopReason":"end_turn"}"#);
                continue;
            }
            if mode == "host-subscriber" || mode == "host-subscriber-permission" {
                for index in 0..160 {
                    update(session, &format!(r#"{{"sessionUpdate":"agent_message_chunk","messageId":"answer","content":{{"type":"text","text":"{index}|"}}}}"#));
                    if index == 7 {
                        if let Ok(gate) = std::env::var("BRIDGE_TEST_SUBSCRIBER_GATE") {
                            std::fs::write(format!("{gate}.ready"), "ready").unwrap();
                            let deadline = std::time::Instant::now() + Duration::from_secs(10);
                            while !std::path::Path::new(&format!("{gate}.go")).exists() {
                                assert!(std::time::Instant::now() < deadline, "subscriber gate timed out");
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                if mode == "host-subscriber" {
                    reply(id, r#"{"stopReason":"end_turn"}"#);
                    continue;
                }
            }
            if mode == "host-tree-flood" || mode == "host-tree-burst" {
                let text = "x".repeat(8192);
                let chunks = if mode == "host-tree-burst" { 40 } else { 512 };
                for index in 0..chunks {
                    update(session, &format!(r#"{{"sessionUpdate":"agent_message_chunk","messageId":"answer","content":{{"type":"text","text":"{text}"}}}}"#));
                    if index == 32 {
                        std::fs::write(std::env::var("BRIDGE_TEST_STREAMING").unwrap(), "streaming").unwrap();
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                pending.insert(session.to_owned(), (id.to_owned(), 0, false));
                continue;
            }
            if mode == "prompt-crash" {
                std::process::exit(11);
            }
            if mode == "prompt-error" {
                println!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32000,\"message\":\"fixture prompt error\"}}}}"
                );
                io::stdout().flush().unwrap();
                continue;
            }
            if mode.starts_with("json-") {
                update(session, r#"{"sessionUpdate":"agent_thought_chunk","messageId":"thinking","content":{"type":"text","text":"not a result"}}"#);
                update(session, r#"{"sessionUpdate":"agent_message_chunk","messageId":"json","content":{"type":"text","text":"{\"count\":"}}"#);
                update(session, r#"{"sessionUpdate":"agent_message_chunk","messageId":"json","content":{"type":"text","text":"3}"}}"#);
                if mode == "json-pending" {
                    pending.insert(session.to_owned(), (id.to_owned(), 0, false));
                    continue;
                }
                if mode == "json-ambiguous" {
                    update(session, r#"{"sessionUpdate":"agent_message_chunk","messageId":"another","content":{"type":"text","text":"{\"count\":3}"}}"#);
                }
                if mode == "json-image" {
                    update(session, r#"{"sessionUpdate":"agent_message_chunk","messageId":"image","content":{"type":"image","data":"YWJj","mimeType":"image/png"}}"#);
                }
                let reason = if mode == "json-truncated" { "max_tokens" } else { "end_turn" };
                reply(id, &format!("{{\"stopReason\":\"{reason}\"}}"));
                continue;
            }
            update(
                session,
                r#"{"sessionUpdate":"agent_message_chunk","messageId":"message-1","content":{"type":"text","text":"Hello "}}"#,
            );
            if mode == "config-fallback" {
                let current = settings.get_mut(session).unwrap();
                current.0 = "model-b".into();
                update(
                    session,
                    &format!(
                        "{{\"sessionUpdate\":\"config_option_update\",\"configOptions\":{}}}",
                        config_options("model-b", current.1)
                    ),
                );
            }
            if mode == "flood" {
                for _ in 0..1024 {
                    update(
                        session,
                        r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}"#,
                    );
                }
            }
            if mode == "permission" || mode == "permissions" || mode == "host-subscriber-permission" {
                let number = if mode == "permissions" { 2 } else { 1 };
                pending.insert(session.to_owned(), (id.to_owned(), number, false));
                update(
                    session,
                    r#"{"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Read fixture","kind":"read","status":"pending"}"#,
                );
                for index in 0..number {
                    let permission =
                        format!("\"permission-{}-{index}\"", session.trim_matches('"'));
                    permissions.insert(permission.clone(), session.to_owned());
                    println!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{permission},\"method\":\"session/request_permission\",\"params\":{{\"sessionId\":{session},\"toolCall\":{{\"toolCallId\":\"tool-1\",\"title\":\"Read fixture\"}},\"options\":[{{\"optionId\":\"allow\",\"name\":\"Allow once\",\"kind\":\"allow_once\"}},{{\"optionId\":\"reject\",\"name\":\"Reject once\",\"kind\":\"reject_once\"}}]}}}}"
                    );
                    io::stdout().flush().unwrap();
                    if let Ok(path) = std::env::var("BRIDGE_TEST_PERMISSION_READY") { std::fs::write(path, "ready").unwrap(); }
                }
            } else if mode == "cancel" || mode == "host-tree-cancel" {
                pending.insert(session.to_owned(), (id.to_owned(), 0, false));
            } else {
                update(
                    session,
                    r#"{"sessionUpdate":"agent_message_chunk","messageId":"message-1","content":{"type":"text","text":"world"}}"#,
                );
                gate("BRIDGE_TEST_COMPLETION_GATE");
                reply(id, r#"{"stopReason":"end_turn"}"#);
                if let Ok(path)=std::env::var("BRIDGE_TEST_COMPLETION_SENT") {std::fs::write(path,"sent").unwrap();}
            }
        } else if line.contains("\"method\":\"session/cancel\"") {
            let session = scalar(&line, "sessionId");
            if let Some((id, _, _)) = pending.remove(session) {
                update(
                    session,
                    r#"{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"failed"}"#,
                );
                reply(&id, r#"{"stopReason":"cancelled"}"#);
            }
        } else if line.contains("\"result\"") {
            let response_id = scalar(&line, "id");
            if let Some(session) = permissions.remove(response_id) {
                gate("BRIDGE_TEST_DECISION_GATE");
                if let Some((_, remaining, cancelled)) = pending.get_mut(&session) {
                    *remaining -= 1;
                    *cancelled |= line.contains("\"outcome\":\"cancelled\"");
                    if *remaining == 0 {
                        let (id, _, cancelled) = pending.remove(&session).unwrap();
                        if !cancelled {
                            update(
                                &session,
                                r#"{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed"}"#,
                            );
                        }
                        reply(
                            &id,
                            if cancelled {
                                r#"{"stopReason":"cancelled"}"#
                            } else {
                                r#"{"stopReason":"end_turn"}"#
                            },
                        );
                    }
                }
            }
        }
    }
}

fn gate(variable: &str) {
    if let Ok(path) = std::env::var(variable) {
        std::fs::write(format!("{path}.ready"),"ready").unwrap();
        let deadline = std::time::Instant::now()+Duration::from_secs(10);
        while !std::path::Path::new(&format!("{path}.go")).exists() {
            assert!(std::time::Instant::now()<deadline,"fixture gate timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn config_options(model: &str, flag: bool) -> String {
    let effort = if model == "model-a" { "high" } else { "low" };
    format!(r#"[
      {{"id":"model","name":"Model","category":"model","type":"select","currentValue":"{model}","options":[{{"group":"provider","name":"Provider","options":[{{"value":"model-a","name":"Model A"}},{{"value":"model-b","name":"Model B"}}]}}]}},
      {{"id":"effort","name":"Effort","category":"thought_level","type":"select","currentValue":"{effort}","options":[{{"value":"{effort}","name":"{effort}"}}]}},
      {{"id":"toggle","name":"Toggle","type":"boolean","currentValue":{flag}}}
    ]"#).replace('\n', "")
}
