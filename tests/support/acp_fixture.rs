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
    } else {
        // Normal ACP shutdown closes stdin. No invented shutdown RPC is required.
        for line in input.lines() {
            if line.is_err() {
                break;
            }
        }
    }
}
