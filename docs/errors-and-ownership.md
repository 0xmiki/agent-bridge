# Errors and ownership

Working contract at H1 acceptance. These rules describe the current implementation;
they do not promise semver stability or automatic recovery from unknown outcomes.

## What owns what

The Rust connection owns the ACP process and SDK task. A native session borrows the
connection; a run exclusively borrows its session; a recorded run also borrows its
store. Distinct direct Rust sessions can share a connection, so a connection failure
can affect all of them. The host instead creates one connection per owned session.

The host owns provider/session supervisors, bounded storage workers, and its stdio
transport. The Bun client owns that host subprocess. Individual event subscriptions
own only their buffers. Database records outlive these objects. A provider's native
history is separate from bridge records and from a client projection checkpoint.

| Action | Meaning | What it does not establish |
| --- | --- | --- |
| Close/break an observer | Detach that observer | Cancellation or provider completion |
| Request cancellation | Send cancellation intent | A confirmed cancelled outcome; completion may win |
| Receive `run_finished` | Host reports a terminal interpretation and any recording error | Successful application validation or native instruction/skill enforcement |
| Drop an unfinished Rust run | Request cancellation and retire that native session handle | That the provider has stopped or the transcript is complete |
| Acknowledge `shutdown` | Host accepted shutdown intent | Process exit; await `host.close()` |
| Delete a provider session | Invoke the provider's deletion semantics | Identical retention behavior across providers; Codex's pinned adapter archives |

If the application needs a confirmed cancellation outcome, cancel and consume the
run before shutting down the host. Closing a host with active work may reject their
pending completion promises rather than deliver final run events. Best-effort stored
interruption evidence is not a provider acknowledgment.

`AcpConnection::shutdown()` has a three-second outer cleanup timeout. Prefer it to
Drop, which requires the executor to remain alive long enough for SDK cleanup. The
example preserves generation and cleanup failures instead of returning early before
attempting shutdown. Linux process-group tests do not establish Windows support or
termination during uninterruptible kernel I/O.

## Host and client failure categories

The wire uses request-scoped `{ ok: false, error: { code, message } }` responses.
The Bun client rejects the corresponding promise and retains the code on the error.
Some errors below originate in the local client rather than the wire. Messages
provide diagnostics; do not parse them to decide whether rerunning an agent
is safe. Unknown error codes must remain visible rather than becoming success.

| Failure | Application response |
| --- | --- |
| `invalid_params`, `unknown_method`, `unsupported_protocol` | Correct the request or client version. No compatibility fallback is implied. |
| `setup_failed`, `session_setup_failed` | Inspect launch/setup diagnostics. A timed-out native setup may have created an unreturned session. |
| `start_failed` | Do not infer that no work or records exist. Run registration/input persistence may already have happened; this code alone is not a retry guarantee. |
| `session_busy`, `stale_run`, `not_running`, `unknown_session` | Reconcile the application's live handles with their current ownership. |
| `invalid_response` | Refresh current pending permissions. Invalid/stale input must not consume a different valid request. |
| `cancel_failed` | The cancellation call failed; it is not evidence that execution stopped. Observe the run/connection outcome. |
| `session_limit`, `history_limit`, `session_unavailable` | Respect bounded admission. Do not create an unbounded retry loop. |
| `history_failed` | Keep the last committed projection/cursor pair; retry the read when its cause is resolved. It must not trigger generation. |
| `subscriber_lagged`, `subscriber_limit` | Handle the affected local observer. Reattach and synchronize saved state; query live pending permissions separately. |
| `invalid_tools`, `invalid_tool_grant`, `tools_locked` | Correct declarations/grants before session creation. Catalogs and bindings are immutable. |
| `invalid_tool_result` | The invocation, scope, or result is invalid, stale, or settled. Do not resend it as another invocation. |
| `tool_binding_retired` | An invocation became uncertain. Reconcile its effects before explicitly creating a fresh session/binding. |
| Protocol corruption, host exit, or client request timeout | Treat the shared connection as failed. Active runs may be uncertain; do not automatically replay them. |

`run_finished.status` and `reason` must be considered alongside `recording_error`.
The host conservatively reports `unknown` after recording/adapter errors. Direct
Rust callers receive `RecordingError` and can inspect the underlying `AcpError` or
`StoreError`. The host's string diagnostic is not a lossless serialization of every
Rust error variant. `session_error` currently carries diagnostics; the Bun client
does not expose it as a separate application event. A failed requested provider
cleanup makes host close fail through a nonzero exit.

## Storage and state

| Storage condition | Contract |
| --- | --- |
| `Busy` | SQLite could not obtain/complete the required lock operation within its budget. Do not infer whole-run rollback. |
| `StorageTimedOut` | The host stopped waiting. An already-started mutation can commit later. The affected handle is retired. |
| `StorageUnavailable` | The worker/handle is unusable. Later calls cannot silently restart it. |
| `StorageOverloaded` | That operation was not admitted. Earlier operations in the same workflow may still exist. |
| `ReadOnly`, `SchemaMigrationRequired`, unsupported schema | Use the correct store owner/version. Read endpoints never create or migrate databases. |
| Invalid change cursor | Discard an incompatible projection/cursor pair and rescan when appropriate; do not substitute a creation-sequence cursor. |

The [host worker policy](runtime-isolation.md) bounds caller waits, not the duration
of kernel I/O. Work that has not begun is skipped after timeout; work already in
progress is uncertain. Direct Rust storage calls remain synchronous and do not
inherit the host policy.

Snapshots and changes are latest-record upserts, not an event stream for executing
effects. Apply revisions monotonically, preserve record identity, and commit state
and cursor together. `SessionState` does this for the Bun client. See
[state synchronization](state-sync.md) for paging and checkpoint version behavior.

Typed [receipt readers](receipt-readers.md) distinguish prepared, attempted, observed,
unknown, valid, rejected, and unsupported evidence. Missing receipts do not establish
success. Reading a receipt does not replay context, reconstruct a validator, or grant
tool authority. Hosted crash reconciliation and explicit recovery choices remain H3.

## Compatibility boundaries

The stdio wire uses version 1 and rejects unsupported versions. New commands or
optional fields can be added without changing existing meanings. A new unsolicited
event kind needs negotiation or a wire-version change because the current client
rejects unknown events. There is no capability-negotiation layer for these additions
yet. Breaking host/client changes therefore require coordinated updates.

Database schema versions, inner receipt versions, and projection checkpoint versions
are independent. Unknown receipt versions remain visible as unsupported evidence;
a future checkpoint version is rejected. Neither behavior should be converted into
an automatic provider retry. H4 still requires packaged-client and upgrade evidence.
Hosted tools negotiate callback protocol 1 before producing tool control events.
Result acknowledgments confirm queuing; invocation receipts record persisted outcomes.
See [hosted tools](host-tools.md).
