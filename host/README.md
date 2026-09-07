# Host integration lab

An experimental Rust subprocess and Bun TypeScript client for testing agent-bridge
as an application dependency. It is private development tooling, not a released
package or network service. Configure trusted local executables and paths.

Install Bun separately (tested with 1.3.13); the Nix shell supplies the Rust toolchain.

```sh
nix-shell
cargo build --features host --bin agent-bridge-host
cd host
bun install --frozen-lockfile
bun run typecheck
bun test
cd ..
bun host/example.ts /tmp/bridge.sqlite3 /absolute/workspace /absolute/opencode acp
```

The example runs two conversational turns, requests cancellation on a third, then
restarts the host and reads saved history without launching a provider. Completion
may win a cancellation race. Provider authentication and model setup are external.

```ts
import { BridgeHost } from "./host/client";

const host = new BridgeHost("./target/debug/agent-bridge-host");
try {
  const session = await host.createSession({
    database: "/tmp/bridge.sqlite3",
    workspace: "/absolute/workspace",
    executable: "/absolute/opencode",
    args: ["acp"],
  });
  const run = session.run("Say hello without tools.");
  for await (const event of run.events) {
    if (event.event === "text_delta") process.stdout.write(event.text);
    if (event.event === "permission") await run.respond(event.permission_id, null);
  }
  console.log(await run.completed);
  console.log(await session.history());
} finally {
  await host.close();
}
```

Drain a run's single-consumer event stream, or cancel it and drain the outcome.
Permission dismissal in this example is an application choice. Real applications
can present the offered options. A cancellation acknowledgment records intent;
the terminal event determines the outcome.

## Wire contract

Version 1 uses JSON lines on stdin/stdout, with stderr reserved for diagnostics.
Requests contain `version`, `id`, `method`, and `params`. Responses carry the same
ID and either `ok: true, result` or `ok: false, error`. Events carry `event`;
run events also carry the original request ID as `stream`, plus session/run IDs.

Methods: `ping`, `create_session`, `run`, `cancel`, `respond`, `history`, `shutdown`.
The host owns identifiers and permission routing. The client imports no ACP SDK.
`client.ts` defines the current DTOs. Record sequence/revision and pagination cursors
are decimal strings to preserve precision. Payload data remains an untyped portable
envelope; typed projections are a future gate.

## Ownership and limits

Each session owns a worker, runtime, ACP process, and SQLite connection. One run is
active per session. Initial limits are eight sessions, sixteen queued commands per
session, 128 output frames, 32 queued input frames, 1 MiB input frames, four concurrent
history reads, and 1,000 records per page. The client permits 64 pending requests,
uses a 45-second response timeout, and bounds each run stream to 128 events.
Overflow fails explicitly; these are development limits, not a production QoS promise.

Normal shutdown cancels workers and joins owned processes. Separate workers contain
some failures but synchronous SQLite work can still delay that session's cancellation.
A raw client that stops reading stdout can block the writer. Shutdown under storage
and output stalls needs fault tests and implementation work.

History pagination orders record creation, not subsequent changes. Reopening history
does not reconnect a native session or recover uncertain work. Tools, structured
questions, context policies, restoration, and child execution currently have Rust
APIs but are not exposed by this host. Session diagnostics and typed state projections
also need a fuller client contract.

The fixture suite covers concurrent sessions, continuation, cancellation, permissions,
active process close, failed-startup recovery, history reopen, and malformed/versioned requests. Every future
application-facing feature should gain a host acceptance case alongside focused Rust
tests. See the [roadmap](../milestone.md) and [quality gates](../docs/quality-gates.md).
