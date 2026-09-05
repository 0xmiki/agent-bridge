# ACP adapter notes

The adapter covers connection lifetime, new sessions, text prompts, streamed
updates, permission decisions, and cancellation. It is not the final provider
interface. ACP details stay in `agent_bridge::acp`; the core model remains
independent of them.

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
The process inherits the host's directory and environment. Each new session supplies
its own absolute workspace path. Launch configuration is not persisted.

`info()` returns the SDK's initialization response, including agent information,
authentication methods, and advertised capabilities. It does not establish login
state, model access, or that a capability works correctly in a real workflow.
We deliberately do not turn these declarations into core execution guarantees yet.

## Sessions and runs

Create a native session associated with application-owned IDs, then consume a run:

```rust
use agent_bridge::{RunId, SessionId, SlotId};
use agent_bridge::acp::{AcpConnection, AcpEvent};

async fn ask(connection: &AcpConnection) -> Result<(), Box<dyn std::error::Error>> {
    let mut session = connection.new_session(
        SessionId::new("conversation-1")?,
        SlotId::new("local-provider")?,
        std::env::current_dir()?,
        vec![], // Optional existing MCP server configurations.
    ).await?;

    let mut run = session.start_run(RunId::new("run-1")?, "Explain this project.")?;
    while let Some(event) = run.next().await? {
        match event {
            AcpEvent::Permission { id, .. } if run.permission_pending(&id) => run.respond(id, None)?,
            AcpEvent::Update(update) => println!("{update:?}"),
            AcpEvent::Finished(reason) => println!("{reason:?}"),
            _ => {},
        }
    }
    Ok(())
}
```

The snippet dismisses permissions. Real applications can show each request's offered
options and call `run.respond(id, Some(option_id))`. Invalid options leave the request
pending. Permission IDs include their run identity so an earlier run's decision
cannot resolve a later run's request. Callers must assign unique run IDs.
Resolution events also report automatic cancellations. Check `permission_pending`
before answering a queued request that may already have been resolved.

One run exclusively borrows its session. After consuming its terminal result and
dropping the run handle, another run can continue the same native session. Distinct
native sessions can execute concurrently through the connection. Application and
native session IDs remain separate; multiple native sessions may contribute to one
application session. `session.info()` exposes the initial native configuration.

`run.run()` exposes the shared run state and IDs; `run.input()` exposes its new text
input. The run starts in `Starting`, becomes `Running` when activity is observed,
and settles when the prompt response arrives. A peer error fails the run. A lost
connection leaves its outcome `Unknown`. A finished execution is not necessarily
a successful task: callers must preserve stop reasons such as refusal or token limits.

`run.cancel()` records cancellation intent, dismisses pending permissions, and sends
`session/cancel`. Continue consuming `next()` for late updates and the provider's
terminal response. New permission requests during cancellation are dismissed too.
There is no default generation deadline; applications can apply a timeout. The
`acp_chat` example limits setup and generation to 60 seconds.

Dropping an unfinished run requests cancellation and retires its session handle.
No subsequent run can accidentally consume late traffic from the abandoned prompt.
This does not establish that the provider stopped. Explicit connection shutdown
terminates the owned process. Cancelling only a pending `next()` wait is safe and
does not abandon the run, so it can be used in an application event loop.

There are 256 queued events and at most 32 pending permissions per run. Handlers do
not block protocol dispatch while waiting for a user. Overflow produces an explicit
error and terminates the connection; other unfinished runs then have unknown
outcomes. This is deliberately conservative until we implement finer recovery.
These limits bound item counts, not individual protocol message sizes.

MCP stdio configurations require absolute executable paths. HTTP/SSE configurations
require the corresponding advertised capability. Configuration is forwarded to
the agent; it does not grant permission by itself or implement an MCP server.

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
- Session fixtures verify ordered streaming, repeated turns, concurrent session
  routing, MCP configuration delivery, explicit permission choices, stale-decision
  rejection, cancellation, failed prompts, abandoned runs, and queue overflow.
- A local handshake with OpenCode 1.18.25 succeeded. It advertised image input,
  embedded context, session loading/resume, and HTTP/SSE MCP support. The example
  then shut down successfully.
- A real text run in a temporary workspace streamed `agent-bridge connected.` and
  returned `EndTurn` through OpenCode. No tool use was requested. This does not yet
  validate real MCP execution, permission behavior, or other providers.

The fixture is a standalone Rust program compiled by the integration test using
`rustc`, so tests need no installed AI CLI, credentials, Python, or network access.
Fixture artifacts stay under Cargo's target directory.

## Next questions

Native session history stays with the provider. The adapter sends only the new
text and does not replay application history, resolve context manifests, or store
continuation handles. The current empty core context manifest means no explicit
record/resource selection was supplied; it does not mean the native session has
no history. The optional [recorded-run wrapper](records.md) now assembles portable
records in a memory or [SQLite store](sqlite.md). Persisted transcripts do not restore
the provider's native session; continuation recovery remains open.

Model selection, instruction injection, image input, native resume/close, structured
output, and other client requests remain outside this step. Updates outside active
runs are not retained. Capability tests must establish those behaviors before the
adapter claims support. The session-creation timeout is currently 30 seconds; timed
out requests are not automatically retried or adopted later.

## References

- [ACP Rust SDK 2.1.0](https://docs.rs/agent-client-protocol/2.1.0/agent_client_protocol/)
- [SDK subprocess implementation](https://docs.rs/crate/agent-client-protocol/2.1.0/source/src/acp_agent.rs)
