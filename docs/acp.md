# ACP connection notes

This first adapter step covers connection lifetime and initialization only. It is
not the final provider interface. ACP details stay in `agent_bridge::acp`; the
slot/session/run/record model remains independent of them.

## Usage

Enable the `acp` Cargo feature and call from a Tokio runtime:

```rust
use agent_bridge::acp::{AcpConnection, AcpLaunch};

async fn inspect() -> Result<(), agent_bridge::acp::AcpError> {
    let connection = AcpConnection::connect(
        AcpLaunch::new("opencode").arg("acp"),
    ).await?;

    let info = connection.info();
    println!("Agent: {:?}", info.agent_info);
    connection.shutdown().await
}
```

`AcpLaunch` accepts an executable, individual arguments, environment overrides,
and an initialization timeout. The default timeout is 15 seconds. Arguments are
passed directly without shell parsing. No automatic installation or login occurs.
The process inherits the host's directory and environment; per-session workspace
selection will come with session creation. Launch configuration is not persisted.

`info()` returns the SDK's initialization response, including agent information,
authentication methods, and advertised capabilities. It does not establish login
state, model access, or that a capability works correctly in a real workflow.
We deliberately do not turn these declarations into core execution guarantees yet.

## Lifecycle

The connection owns a task that drives the official SDK's ACP v1 transport.
`connect()` returns after initialization and protocol-version validation. It
advertises no filesystem or terminal access because we have not implemented those
client operations. This does not sandbox the provider process itself.

`shutdown().await` ends the protocol scope and waits for SDK cleanup, with a
three-second outer timeout. `wait_closed().await` observes spontaneous closure or
failure. `is_closed()` is only a snapshot of the local task, not a health probe.

Dropping a connection or cancelling its pending connect future aborts the task.
The executor must remain alive long enough to drop the SDK connection and clean up
the process. Prefer explicit shutdown. On Unix, SDK cleanup terminates the owned
process group. Windows descendant cleanup is not covered by our current tests.

Initialization timeouts abort and await the task before returning. Missing
executables, protocol errors, and child failures return typed errors. The SDK may
include bounded stderr diagnostics in an error; applications should review those
before logging. Agent-bridge does not log protocol traffic or environment values.

## Evidence so far

- Subprocess fixtures test initialization, unsupported versions, malformed
  responses, missing executables, early exit, post-initialization failure, literal
  arguments, stderr flooding, timeout, drop, cancelled connect, and shutdown.
- Linux tests also verify process exit and cleanup of wrapper descendants.
- A local handshake with OpenCode 1.18.25 succeeded. It advertised image input,
  embedded context, session loading/resume, and HTTP/SSE MCP support. The example
  then shut down successfully. No session or model execution was requested.

The fixture is a standalone Rust program compiled by the integration test using
`rustc`, so tests need no installed AI CLI, credentials, Python, or network access.
Fixture artifacts stay under Cargo's target directory.

Next: create an ACP session, stream a prompt through a run, and route permissions
through explicit application decisions. We still need capability tests for real
tool calls, history, instructions, model switching, and structured output.

## References

- [ACP Rust SDK 2.1.0](https://docs.rs/agent-client-protocol/2.1.0/agent_client_protocol/)
- [SDK subprocess implementation](https://docs.rs/crate/agent-client-protocol/2.1.0/source/src/acp_agent.rs)
