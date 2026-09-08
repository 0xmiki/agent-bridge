# Application integration

This is the working H1 integration contract. The host and public APIs remain
experimental; revise this guide when an actual consumer exposes a better boundary.

## Choose the ownership boundary

| Path | Use it when | What the application owns |
| --- | --- | --- |
| Rust host plus Bun client | You want the demonstrated hosted lifecycle and storage/observer limits | Host lifetime, presentation, permission choices, polling and checkpoint persistence |
| Direct Rust library | You want to embed the adapter and choose your runtime/storage arrangement | Connection and session scopes, run IDs, event loop, execution deadlines, blocking-storage isolation |

Both use the same slots, runs, records, cursors, and receipts. Neither requires an
application to implement ACP parsing or a subprocess supervisor. The direct Rust
path deliberately exposes typed ACP adapter events. It is not a Rust version of the
Bun client, and it does not automatically inherit the host's isolation policy.

The host currently has no released Rust client, Node/browser client, Tauri package,
or remote service. The TypeScript client is Bun-specific. An application using the
stdio contract from another runtime must own that transport integration until a
packaged client exists. H4 covers package installation and independent consumers.

## Hosted application

Build the host and the documented Rust example used by its acceptance suite:

```sh
nix-shell
cargo build --locked --features host --bin agent-bridge-host --example rust_integration
cd host
bun install --frozen-lockfile
bun run typecheck
bun test
```

See [host/README.md](../host/README.md) for a complete client example. The ordinary
flow is create session, run, consume events, handle permissions, and observe the
terminal outcome. `run.events` is an observer; `run.completed` is the outcome.
Closing an observer does not cancel work. See [subscriptions](subscriptions.md).

Use `SessionState.sync(host)` to build a transcript from snapshots and coalesced
changes. Save its checkpoint as one value, including cursor and projection version.
Do not replace the change cursor with the last record's creation sequence. A new
host can read that state without launching another provider. This is saved-state
recovery, not native-session resume or durable execution.

## Direct Rust application

The complete, compiled example is [rust_integration.rs](../examples/rust_integration.rs):

```sh
cargo run --features acp,sqlite,receipts --example rust_integration -- \
  /tmp/bridge-rust.sqlite3 /absolute/workspace /absolute/opencode acp
```

The example allocates fresh application IDs, takes a starting change cursor, launches
one provider, records a prompt, dismisses permissions, and preserves its stop reason.
It attempts optional test-session cleanup and explicit connection shutdown even when
generation fails. After successful execution it compares the change projection with
history, reads typed receipts, and verifies the same records through a reopened store.
It does not restart generation or resume provider context during the readback.

For a pinned Codex ACP adapter, pass its executable or Node script in the same way.
The example recognizes codex-acp launches and requests test-session deletion on exit;
set `AGENT_BRIDGE_CODEX_TEST=1` for a custom wrapper. The pinned adapter archives the
thread out of active history. This cleanup is for disposable tests, not ordinary
application conversations, and cannot cover a native ID lost during failed setup.

The direct example is a CLI, with synchronous SQLite operations. Its 60-second
generation timeout cannot preempt blocked storage or stdout calls. In a desktop
application, keep these operations off the UI thread and choose a storage execution
boundary explicitly. The host's bounded storage workers are implemented in the host,
not exported as a general-purpose Rust runtime. Do not describe this direct example
as having the host's 500 ms storage deadlines.

## API correspondence

| Operation | Bun client | Direct Rust |
| --- | --- | --- |
| Own execution | `BridgeHost`, `createSession` | `AcpConnection::connect`, `new_session` |
| Assign identifiers | Host assigns session, slot, and run IDs | Application constructs `SessionId`, `SlotId`, `RunId` |
| Record work | `session.run` | `session.start_recorded_run` with a `RecordStore` and `RecordActors` |
| Observe | `run.events`, `run.subscribe`, `run.completed` | Drive `RecordedRun::next`; inspect `run().status()` and stop reason |
| Cancel | `run.cancel`, then await outcome | `run.cancel`, then continue `next()` |
| Respond | `run.respond`, `pendingPermissions` for reattachment | `permission_pending`, `respond` on the borrowed run |
| Read saved state | `SessionState`, `snapshot`, `changes`, `history` | `ChangeStore::snapshot_page/changes`, `RecordStore::list` |
| Read receipts | `readReceipt(record)` or receipt projection items | `records::receipts::read(&payload)` |
| Finish ownership | `host.close` | Drop the run/session, then `connection.shutdown().await` |

Driving `RecordedRun::next` is necessary for recording. A live Rust `snapshot()` is
not a replacement for driving events or checking durable store state. One run borrows
its native session exclusively; drop the finished run before another turn.

## Current support boundary

Linux has host lifecycle evidence. OpenCode and Codex have recorded live checks for
the documented versions; Claude authenticated checks remain deferred. A protocol
capability declaration alone is not compatibility evidence. See
[provider compatibility](providers.md) and [host verification](../host/verification.md).

Application tools now have a [hosted workflow](host-tools.md). Structured questions,
selected context, and validated results have Rust building blocks and remain H2
host work. Native resume and
portable restoration exist in Rust, while hosted restart reconciliation is H3.
Review the [error and ownership contract](errors-and-ownership.md) before implementing
retry, cancellation, or application shutdown behavior.
