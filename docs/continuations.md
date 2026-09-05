# Provider continuations

Working implementation, not a general recovery system. A continuation is a
single-use handoff for provider-owned context. It complements portable records;
it does not replace them or turn hidden provider state into interchangeable data.

## Mental model

```text
application session
    ├── portable records
    └── provider continuation A, available
            │ claim + native resume
            ▼
        provider continuation A, claimed
            │ successful work + handoff
            ▼
        provider continuation B, available
```

The application session keeps its identity while a native provider session moves
between process connections. Each handoff records the application session, slot,
adapter, provider scope, native key, predecessor, and adapter-owned compatibility
data. It does not store credentials, MCP environment values, or a transcript copy.

`ContinuationStore` extends the local record store and works with memory or SQLite.
An available handle can be claimed once. A claimed handle cannot become available
again. After a successful resumed session finishes its work, `handoff` creates a new
available successor and marks the predecessor as no longer latest.

This chain prevents two workers from intentionally resuming the same saved handle.
It does not lock the provider's own session against other software, detect changes
made outside agent-bridge, or prove the provider restored the expected hidden state.

## ACP usage

Set a stable scope that identifies the provider account and state directory used by
the launched process:

```rust
use agent_bridge::{ContinuationId, SessionId, SlotId};
use agent_bridge::acp::{AcpConnection, AcpLaunch};
use agent_bridge::records::SqliteStore;

let store = SqliteStore::open("application.sqlite3")?;
let launch = AcpLaunch::new("opencode")
    .arg("acp")
    .continuation_scope("default-profile");

let connection = AcpConnection::connect(launch.clone()).await?;
let session = connection.new_session(
    SessionId::new("conversation-1")?,
    SlotId::new("local-opencode")?,
    std::env::current_dir()?,
    vec![],
).await?;
session.handoff(ContinuationId::new("handoff-1")?, &store)?;
connection.shutdown().await?;

let connection = AcpConnection::connect(launch).await?;
let session = connection.resume_saved(
    &store,
    &ContinuationId::new("handoff-1")?,
    vec![],
).await?;
```

`handoff` consumes an idle session handle. It requires native ACP resume support and
a nonempty scope. A session with abandoned, failed, or uncertain work cannot produce
a handoff. The source process can then shut down without discarding the saved native
locator.

`resume_saved` uses ACP `session/resume`. It never replays application records, calls
`session/load`, starts a fresh provider session, or retries. The caller supplies MCP
servers again because the ACP resume request requires the intended configuration.

The current ACP adapter requires the same scope, adapter family, agent name, agent
version, and absolute working directory stored during handoff. Exact agent-version
matching is conservative. We may loosen it after compatibility testing, but the
package should not guess that an upgrade preserves native session semantics.

## Claim boundary

Compatibility and MCP validation happen before claim. These failures leave the saved
handle available. Once agent-bridge claims the handle, it sends the resume request.
A timeout, provider error, lost connection, or process crash after that point leaves
the handle claimed forever because the provider may already have resumed or changed
the native session.

Claims do not expire automatically. Time-based leases would let another worker retry
an action whose outcome is unknown. A future reconciliation API needs affirmative
provider evidence before it can make a claimed handle reusable.

The continuation descriptor and native key are opaque application data. They are not
credentials, but applications should avoid exposing them in analytics or user-facing
logs. Provider credentials remain in the provider's own authentication mechanism.

## What has been verified

- Memory and SQLite share tests for immutable descriptors, single claims, successor
  chains, missing sessions, and concurrent claims.
- SQLite preserves continuation state across reopen and migrates a version 1 records
  database to schema version 2 without losing existing records.
- ACP fixtures cover scope and version mismatch, unsupported resume, invalid MCP
  configuration, provider rejection, timeout, one-time claims, and SQLite reopen.
- OpenCode 1.18.25 preserved a unique phrase across a real ACP process restart. The
  second process used `session/resume` and returned the exact phrase. This verifies
  one native-resume path, not other providers or cross-provider transfer.

## Still open

- Discovering which provider upgrades can safely resume older native sessions.
- Recording an affirmative provider checkpoint or synchronization boundary.
- Reconciling a claimed handle after a host crash when the provider can report state.
- Linking produced handoffs to completed work. Runs now store their native session's
  originating continuation without re-claiming it for every turn.
- Choosing and restoring portable context when native resume is unavailable.
- Close, load-with-replay, fork, and provider-managed subagent relationships.

The runnable `acp_resume` example performs a two-process handoff and checks a unique
memory phrase. It makes real model calls:

```sh
cargo run --features acp,sqlite --example acp_resume -- \
  /tmp/agent-bridge.sqlite3 /absolute/workspace opencode acp
```
