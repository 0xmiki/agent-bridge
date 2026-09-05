#![cfg(feature = "mcp")]
use agent_bridge::mcp::McpToolServer;
use agent_bridge::tools::*;
use agent_bridge::{ActorId, SessionId, SlotId};
use rmcp::ServiceExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    value: u32,
}

#[tokio::test]
async fn mcp_lists_only_granted_tools_and_uses_host_scope() {
    let mut registry = ToolRegistry::default();
    for name in ["allowed", "hidden"] {
        registry
            .register::<Input, _, _, _>(
                ToolRef {
                    name: name.into(),
                    revision: "v1".into(),
                },
                "Read scoped value",
                |context, input| async move {
                    Ok(json!({"value":input.value,"session":context.scope.session.as_str()}))
                },
            )
            .unwrap();
    }
    let started = Arc::new(tokio::sync::Notify::new());
    let stopped = Arc::new(tokio::sync::Notify::new());
    let start = started.clone();
    let stop = stopped.clone();
    struct Guard(Arc<tokio::sync::Notify>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    registry
        .register::<Input, Value, _, _>(
            ToolRef {
                name: "wait".into(),
                revision: "v1".into(),
            },
            "Wait until cancelled",
            move |_, _| {
                let start = start.clone();
                let stop = stop.clone();
                async move {
                    let _guard = Guard(stop);
                    start.notify_one();
                    std::future::pending().await
                }
            },
        )
        .unwrap();
    let host = ActorId::new("host").unwrap();
    let actor = ActorId::new("agent").unwrap();
    let scope = ToolScope {
        session: SessionId::new("trusted-session").unwrap(),
        slot: SlotId::new("slot").unwrap(),
    };
    let grant = ToolGrant {
        issuer: host.clone(),
        subject: actor.clone(),
        scope: scope.clone(),
        tools: vec![
            ToolRef {
                name: "allowed".into(),
                revision: "v1".into(),
            },
            ToolRef {
                name: "wait".into(),
                revision: "v1".into(),
            },
        ],
    };
    let server = McpToolServer::new(Arc::new(registry), grant, scope, actor, host).unwrap();
    let (client, transport) = tokio::io::duplex(16384);
    let serving = tokio::spawn(async move {
        server
            .serve(transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    async fn exchange(
        read: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        write: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
        message: Value,
    ) -> Value {
        let id = message["id"].clone();
        write
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        loop {
            let mut line = String::new();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(3), read.read_line(&mut line))
                    .await
                    .unwrap()
                    .unwrap()
                    > 0
            );
            let reply: Value = serde_json::from_str(&line).unwrap();
            if reply["id"] == id {
                return reply;
            }
        }
    }
    let initialized = exchange(&mut read, &mut write, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).await;
    assert!(initialized.get("result").is_some());
    write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    let catalog = exchange(
        &mut read,
        &mut write,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    assert_eq!(catalog["result"]["tools"].as_array().unwrap().len(), 2);
    assert_eq!(catalog["result"]["tools"][0]["name"], "allowed");
    let called = exchange(&mut read, &mut write, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"allowed","arguments":{"value":7},"_meta":{"session":"spoofed"}}})).await;
    let value: Value =
        serde_json::from_str(called["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(value, json!({"value":7,"session":"trusted-session"}));
    for (id, name, arguments) in [
        (4, "hidden", json!({"value":7})),
        (5, "allowed", json!({"value":"bad"})),
    ] {
        let result = exchange(&mut read, &mut write, json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})).await;
        assert_eq!(result["result"]["isError"], true);
    }
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/call\",\"params\":{\"name\":\"wait\",\"arguments\":{\"value\":0}}}\n").await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":6,\"reason\":\"test\"}}\n").await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), stopped.notified())
        .await
        .unwrap();
    assert!(
        exchange(
            &mut read,
            &mut write,
            json!({"jsonrpc":"2.0","id":7,"method":"ping"})
        )
        .await
        .get("result")
        .is_some()
    );
    drop(write);
    drop(read);
    tokio::time::timeout(std::time::Duration::from_secs(3), serving)
        .await
        .unwrap()
        .unwrap();
}
