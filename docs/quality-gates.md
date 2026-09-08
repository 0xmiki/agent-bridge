# Integration quality gates

Working acceptance criteria, September 7, 2026. Revise these when an integration
exposes a better requirement. The independent review rated the pre-host architecture
7/10 and roadmap 6/10. These gates are its proposed route toward a defensible 9/10,
not a promise that a test count produces a score. A 10/10 assessment would need
sustained consumer, release, upgrade, and provider-change evidence.
The [H1 acceptance audit](h1-acceptance.md) records which parts of gates 1–3 are
established for the Linux hosted proof and which release/recovery limits remain.

## 1. Application ownership

- Two fixture sessions run concurrently without crossed events; repeat turns keep context.
- Request IDs, malformed input, unsupported versions, and failed startup have explicit outcomes.
- Cancel, EOF, close, and consumer disconnect clean up owned processes and descendants.
- Each advertised provider completes a real host prompt; consumers implement no ACP parser.

Host fixtures force distinct streams to interleave and recall their own prior values.
Tests verify event IDs, ordering, one terminal outcome, cancellation, permissions,
failed-startup recovery, malformed input, and version rejection. Linux fault tests
cover stdin EOF, stdout disconnect, and stalled readers, with host/provider/descendant
exit checked within five seconds. Abrupt host death and other OSes remain open.

## 2. Application state and storage

- Upsert projections agree with final reopened state.
- Snapshot plus change cursor has no race, missing updates, or duplicated effects.
- Updating an old record is visible after reconnect; slow subscribers have bounded,
  documented behavior.
- Bridge-owned receipts have typed readers; applications do not interpret extension JSON.

History tests compare content, attribution, terminal records, and exact snapshots
including IDs/revisions after reopen, through full and paginated reads.
Creation-sequence pagination remains separate from the new SQLite change cursor.
Schema 7 and host `snapshot`/`changes` now support coalesced old-record updates,
overlapping snapshot pages, and persisted client checkpoints. Tests reject foreign
cursors and prove a failed refresh does not advance local state. Initial typed
readers cover common transcript items. [Receipt readers](receipt-readers.md) now cover
all five bridge-owned extension families, with explicit malformed/future handling,
exact host counters, and checkpoint upgrades. See [state synchronization](state-sync.md).

## 3. Runtime isolation

- Hold a SQLite write lock or stall storage during streaming; another session and
  cancellation must progress within defined deadlines.
- Overflow and recording failures are explicit and do not silently corrupt another session.
- A client that stops reading cannot indefinitely prevent shutdown.

An external SQLite write lock now produces an explicit unknown run outcome with a
recording error. A session using another database still starts and cancels within
two-second deadlines. Releasing the lock does not replay the failed run. The host
uses a 100 ms SQLite busy wait per operation; the default Rust store retains five
seconds. Neither setting bounds filesystem I/O or mutex waits.

On Unix, output uses nonblocking writes with a one-second frame deadline. Tests cover
a stalled reader with stdin kept open, disconnect, and EOF under output pressure.
Output failure stops the entire owned host and exits nonzero. This establishes a
bounded failure path, not per-subscriber isolation or lossless backpressure. Arbitrary
kernel-level I/O failures and richer cancellation guarantees remain open.

Shared-database tests now exercise three sessions, interleaved output, simultaneous
projection refreshes, and exact state after reconnect in DELETE and WAL modes.
Host readers use read-only connections and do not take migration write locks.
An external reserved writer lock permits reads of committed state; an exclusive
lock fails a refresh within the deadline, preserving the previous client checkpoint
and host responsiveness. The explicit policy is bounded lock waits and reported
failure, without automatic run retries or a sustained-load fairness claim.

The host now bounds waits on storage workers separately from SQLite's busy timeout.
Controlled blocking-store tests verify cancellation and provider-tree cleanup before
the write returns, then verify that the delayed write can still commit. Initialization
and read timeouts retain actual worker capacity; panics retire handles; queued writes
do not start after timeout. The pool is capped at twelve workers. See
[runtime isolation](runtime-isolation.md) for the exact limits. This does not claim
force-cancellation of OS I/O.

Independent Bun run observers now have bounded event/byte queues. A lagged observer
fails locally while the wire keeps draining and other observers/runs continue. Tests
verify all fast-observer events, saved-state recovery, bounded admission, unsubscribe,
immutable permission options, and reattaching to a pending permission without replay.
The [subscription contract](subscriptions.md) distinguishes this from failure of the
shared transport or fairness between provider output streams.

## 4. Application interactions

- Bind MCP tools mechanically from approved plans and grants.
- Reject spoofed identities, stale revisions, cross-session responses, and expired questions.
- Cancellation reaches application handlers; duplicate decisions have one outcome.
- Exercise a real application tool per supported provider through the host.

Rust tool/question/grant APIs exist. Host tests reject foreign permission tokens,
stale run IDs, invalid options, and duplicate responses while preserving valid pending
requests. Tokens are unique across the host. Hosted tools now derive MCP configuration
from immutable grants, validate runtime schemas, route callbacks outside observer
queues, and persist invocation receipts. Tests cover scope/capability spoofing,
concurrency limits, deadlines, cancellation, recording failures, and rejected retries
after unknown outcomes. OpenCode and Codex passed the live callback/token check.
Hosted questions and the remaining context/result workflows are still open in H2.

## 5. Restart and uncertainty

- Kill the host at dispatch, tool execution, permission decision, and completion boundaries.
- Enumerate unresolved runs and interactions after restart without prior in-memory IDs.
- Never automatically replay an effect with an uncertain outcome.
- Expose explicit recovery choices; verify native resume separately from a fresh run
  with selected portable context.

Saved-history reads do not establish durable execution. These host tests remain open.

## 6. Consumer and release

- Install clean packaged Rust and TypeScript artifacts in an independent consumer.
- Compile the documented API; upgrade a previous database fixture without losing evidence.
- Reject unsupported wire versions with actionable errors.
- Verify launch, cancel, and descendant cleanup on each advertised OS.
- Run deterministic checks in CI and keep real-provider versions and results recorded.

Linux is the only locally exercised host platform. No public alpha or cross-platform
support claim follows from this first host implementation. Claude authenticated checks
remain deferred because no local authenticated installation is available.
