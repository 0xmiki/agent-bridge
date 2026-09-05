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
    if mode == "malformed" {
        println!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":\"bad\"}}}}");
    } else {
        println!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":{version},\"agentCapabilities\":{{\"loadSession\":true,\"promptCapabilities\":{{\"image\":true}}}},\"agentInfo\":{{\"name\":\"fixture\",\"version\":\"1\"}},\"authMethods\":[]}}}}"
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
        "duplicate",
    ]
    .contains(&mode.as_str())
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
            if mode == "new-error" {
                println!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32000,\"message\":\"fixture session error\"}}}}"
                );
                io::stdout().flush().unwrap();
            } else {
                if mode != "duplicate" || count == 0 {
                    count += 1;
                }
                reply(id, &format!("{{\"sessionId\":\"native-{count}\"}}"));
            }
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
            update(
                session,
                r#"{"sessionUpdate":"agent_message_chunk","messageId":"message-1","content":{"type":"text","text":"Hello "}}"#,
            );
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
