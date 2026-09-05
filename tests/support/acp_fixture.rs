//! Standalone std-only subprocess fixture, compiled by the integration tests.
use std::{
    io::{self, BufRead, Write},
    time::Duration,
};

fn main() {
    let mode = std::env::args().nth(1).expect("fixture mode");
    if let Ok(path) = std::env::var("BRIDGE_TEST_PID") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    if let Ok(path) = std::env::var("BRIDGE_TEST_ARGUMENT") {
        std::fs::write(path, std::env::args().nth(2).unwrap()).unwrap();
    }
    let _descendant = if mode == "tree" {
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
        } else if line.contains("\"method\":\"session/prompt\"") {
            let session = scalar(&line, "sessionId");
            let id = scalar(&line, "id");
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
            if mode == "permission" || mode == "permissions" {
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
                }
            } else if mode == "cancel" {
                pending.insert(session.to_owned(), (id.to_owned(), 0, false));
            } else {
                update(
                    session,
                    r#"{"sessionUpdate":"agent_message_chunk","messageId":"message-1","content":{"type":"text","text":"world"}}"#,
                );
                reply(id, r#"{"stopReason":"end_turn"}"#);
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

fn config_options(model: &str, flag: bool) -> String {
    let effort = if model == "model-a" { "high" } else { "low" };
    format!(r#"[
      {{"id":"model","name":"Model","category":"model","type":"select","currentValue":"{model}","options":[{{"group":"provider","name":"Provider","options":[{{"value":"model-a","name":"Model A"}},{{"value":"model-b","name":"Model B"}}]}}]}},
      {{"id":"effort","name":"Effort","category":"thought_level","type":"select","currentValue":"{effort}","options":[{{"value":"{effort}","name":"{effort}"}}]}},
      {{"id":"toggle","name":"Toggle","type":"boolean","currentValue":{flag}}}
    ]"#).replace('\n', "")
}
