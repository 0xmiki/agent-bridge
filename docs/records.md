# Records and local storage

Working implementation. `MemoryStore` is process-local and loses its data when the
application exits. The optional [SQLite adapter](sqlite.md) persists records across
restarts, using schema and JSON format version 1. Future format changes need explicit
compatibility handling even while the public API continues to evolve.

## Recorded runs

The `records` feature provides portable payloads and a local storage contract.
The `acp` feature includes it and adds `start_recorded_run`:

```rust
use agent_bridge::{ActorId, RunId};
use agent_bridge::acp::RecordActors;
use agent_bridge::records::MemoryStore;

// Given an existing AcpSession:
let store = MemoryStore::default();
let mut run = session.start_recorded_run(
    RunId::new("review-1")?, "Review the layout.", &store,
    RecordActors {
        user: ActorId::new("person")?,
        agent: ActorId::new("reviewer")?,
        host: ActorId::new("application")?,
    },
)?;

while let Some(event) = run.next().await? {
    // Render the event and answer any still-pending permission request.
}
```

The store registers run identity and user input before dispatch. Reusing a registered
run ID is rejected before another prompt is sent. A failed startup can leave the ID
registered; retries need a new execution ID rather than assuming no work happened.

After connection shutdown, use `RecordStore::list` to read session history and
`get_run` to resolve execution provenance. Application session, actor, and run IDs
remain independent of native ACP IDs. Record IDs are namespaced by the run ID; the
store assigns session-local sequences in first-observed order.

## Payloads

| Payload | Portable information |
| --- | --- |
| Message | User, agent, or reasoning channel; text content |
| Tool | Title, execution status, structured input and output |
| Permission | Offered choices and their provider-described effects |
| Decision | Selected option or cancellation; local delivery status |
| RunFinished | Completion, refusal, token/step limit, or cancellation |
| Failure | An observed execution or recording error |
| Extension | Namespaced SDK data without a portable representation yet |

Text chunks with a native message ID update one record within a run and channel.
Without IDs, consecutive chunks group until another activity or channel intervenes.
Reasoning and assistant text do not merge. User-message echoes, plans, and unsupported
content remain in ACP extensions rather than becoming duplicate user messages or
disappearing. Tool patches preserve unchanged fields and replace collections.

Native tool details remain under the `acp` extension. Resource extraction is not
implemented yet, so extensions can contain inline content. These are SDK-visible
values, not a lossless copy of original wire messages.

## Checkpoints and immutability

`run.snapshot()` returns live payloads with their persisted identity and latest
checkpoint revision. It explicitly clones content; use the store to read the last
saved snapshot instead.

Text deltas append to a live buffer without a database write per token. The recorder
reserves one open record at first appearance. `run.checkpoint()` saves coarse progress;
completion finalizes records. Failed or abandoned streams preserve partial output
as interrupted. Unfinished tools and unresolved requests are not marked successful.

Finalized records cannot change. Context can therefore reference them without
silently changing past inputs. Summaries should be separate derived records.

## Store guarantees

The synchronous `RecordStore` trait targets local backends. Memory and SQLite share
validation rules and contract tests. Mutations are atomic, including calls through
clones and, for SQLite, independent connections to the same database:

- Stable IDs, unique session ordering, and exclusive-cursor pagination.
- Record ownership matching the registered run's session.
- Idempotent retries of original insertions, even after later checkpoints.
- Revision checks for updates; stale writes and finalized mutations fail.
- One validated permission decision appended atomically with request finalization.

Reads share snapshots through `Arc` instead of copying whole transcripts. The memory
backend retains creation and current snapshots for insertion retries, sharing them
until a checkpoint changes the payload. It does not retain every intermediate delta.
Total retained history is not bounded by this in-memory adapter.

The store validates record references. Resource resolution, access control, deletion,
and cross-session context authorization remain separate work. Run registrations
contain identity and requested core data, not a durable execution scheduler or
confirmed model configuration.

## Decisions and failure boundaries

`AcpEvent::PermissionResolved` reports local submission, including automatic
cancellation. `Queued` means accepted by the local transport queue, not received by
the provider. Check `run.permission_pending(&id)` before answering a streamed request
that might already be resolved.

The recorded wrapper saves requests before exposing them to the caller. Decisions
are recorded as resolution events are consumed. Store writes are not atomic with
external execution or transport delivery. The store's atomic decision operation
does not send a response. Durable intent and delivery recovery need another contract
before disk-backed execution can claim crash safety.

Drain the run to observe recording errors. A storage conflict stops recording and
requests cancellation. Drop only attempts a partial checkpoint; destructors cannot
return storage failures. No background retry proves an unknown action did not happen.

## Next questions

- Future schema and data-version migrations while preserving version 1 files.
- Whether local and remote stores should share an async interface.
- Resource deduplication and retention for large content.
- Atomic execution intent, decision delivery, and crash recovery.
- Portable incremental UI updates without full snapshot reads.

Tests cover record assembly, interruption, decisions, concurrent writes, and duplicate
execution rejection. A real OpenCode run also produced records that remained readable
after the connection shut down.
