# Host integration lab

An experimental Rust subprocess and Bun TypeScript client for testing agent-bridge
as an application dependency. It is private development tooling, not a released
package or network service. Configure trusted local executables and paths.
The [integration guide](../docs/integration.md) compares this path with direct Rust
embedding. The [error/ownership contract](../docs/errors-and-ownership.md) defines
what cancellation, shutdown, observer loss, and storage failures mean.

Install Bun separately (tested with 1.3.13); the Nix shell supplies the Rust toolchain.

```sh
nix-shell
cargo build --features host --bin agent-bridge-host --example rust_integration
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

Each [run subscription](../docs/subscriptions.md) has one consumer. Use
`run.subscribe()` for another observer; `close()` or breaking its loop detaches only
that observer. Lag fails that subscription without cancelling the run or other
observers. `run.completed` remains the execution outcome; use `run.cancel()` to stop work.
Permission dismissal in this example is an application choice. Real applications
can present the offered options. A cancellation acknowledgment records intent;
the terminal event determines the outcome.

## Wire contract

Version 1 uses JSON lines on stdin/stdout, with stderr reserved for diagnostics.
Requests contain `version`, `id`, `method`, and `params`. Responses carry the same
ID and either `ok: true, result` or `ok: false, error`. Events carry `event`;
run events also carry the original request ID as `stream`, plus session/run IDs.

Methods: `ping`, `create_session`, `run`, `cancel`, `respond`, `pending_permissions`, `history`, `snapshot`,
`changes`, `shutdown`.
The host owns identifiers and permission routing. The client imports no ACP SDK.
`client.ts` defines the current DTOs. Record sequence/revision and pagination cursors
are decimal strings to preserve precision. Payload data remains an untyped portable
envelope. [State synchronization](../docs/state-sync.md) adds typed readers in
`state.ts` and a checkpoint containing both records and their change cursor.
[Receipt readers](../docs/receipt-readers.md) in `receipts.ts` expose input, restoration,
validation, and configuration evidence. The host adds validated metadata, represents
malformed/future receipts explicitly, and keeps large counters as decimal strings.

For disposable provider sessions, set `delete_session_on_close: true`. Shutdown
requests ACP session deletion and exits nonzero if cleanup fails. The pinned Codex
adapter maps this to archiving, so the thread leaves active history but is not
permanently erased. Bridge SQLite records remain available for verification.
The example enables cleanup for recognized codex-acp launches; set
`AGENT_BRIDGE_CODEX_TEST=1` for a custom Codex wrapper. This is opt-in for application
sessions. Forced host death or a failure before a native session handle is returned
can still prevent cleanup; general crash recovery is separate work.

## Ownership and limits

Each session owns a worker, runtime, ACP process, and SQLite connection. One run is
active per session. Initial limits are eight sessions, sixteen queued commands per
session, 128 output frames, 32 queued input frames, 1 MiB input frames, four concurrent
history reads, and 1,000 records per page. The client permits 64 pending requests,
uses a 45-second response timeout, and admits eight observers per run. Each has
its own queue bounded by 128 events and 1 MiB of encoded event bytes. A lagged
observer can refresh saved state and query `run.pendingPermissions()` for live
requests it missed.
Overflow fails explicitly; these are development limits, not a production QoS promise.

Storage runs behind a [bounded worker boundary](../docs/runtime-isolation.md): twelve
actual storage workers total, one queued job per session's store, and a 500 ms wait
budget for opening, each record operation, or a read query. Timed-out work retains
its worker slot until it actually finishes. Host writes to the same configured
absolute database path are serialized; other processes and path aliases still use
SQLite's contention behavior.

Normal shutdown cancels workers and joins owned processes. The host waits at most
100 ms per SQLite busy-handler operation, then reports lock contention explicitly.
Recording failure yields `status: "unknown"` with `recording_error`; it retires that
native session and does not replay the prompt. A held lock may also prevent saving
the failure itself. Applications must not infer completion from missing records.
The default Rust store still waits five seconds; callers can choose
`SqliteStore::open_with_busy_timeout`. SQLite's budget does not bound disk I/O or mutex
waits. The host's outer deadline limits caller waiting, but an already-started write
may commit later. `StorageTimedOut` retires the handle; it is not proof of rollback.

`history`, `snapshot`, and `changes` use separate read-only connections and require
an existing, current-schema database. They neither create nor migrate a database.
Reserved writer locks permit reading committed state; exclusive locks may produce
`history_failed` without advancing client state or stopping the host. Three sessions
sharing one database with independent state readers are tested in both DELETE and
WAL journal modes. Applications choose the journal mode; the host does not change it.

Unix stdout writes are nonblocking with a one-second frame deadline. A disconnected
or stalled consumer causes host shutdown and nonzero exit; queued output may be lost.
Linux tests check host, provider, and descendant exit within five seconds, including
EOF under pressure and a stalled consumer that keeps stdin open. This is a whole-host
failure policy for the shared transport. Individual Bun observer lag is isolated
inside the client. Controlled storage-stall tests verify
provider cleanup without waiting for storage to return. Non-Unix pipes and actual
kernel-level I/O failures still need platform verification before wider claims.

History pagination orders record creation; `snapshot` and `changes` support record
updates and reconnecting projections. Reopening history
does not reconnect a native session or recover uncertain work. Tools, structured
questions, context policies, restoration, and child execution currently have Rust
APIs but are not exposed by this host. Session diagnostics and typed state projections
also need a fuller client contract.

The fixture suite forces distinct streams to interleave and retain their own context,
compares exact history after restart, tests permission rejection without consuming valid
requests, and exercises SQLite locks and consumer faults. It also covers cancellation,
active process close, failed startup, and malformed/versioned requests. Every future
application-facing feature should gain a host acceptance case alongside focused Rust
tests. See the [roadmap](../milestone.md) and [quality gates](../docs/quality-gates.md).
