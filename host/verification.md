# Host verification

September 7, 2026, Linux/NixOS, Bun 1.3.13. These results apply to the experimental
host in this change, not to every feature of the Rust library.

At the initial host commit `511c090`, local deterministic checks passed: 175 Rust tests (`--all-features --all-targets`),
Clippy with warnings denied, rustfmt, default-feature-free compilation, TypeScript
typechecking, and five Bun integration tests (18 assertions). Linux CI is configured;
these are local results, not a claim about a remote CI run.

| Provider | Versions | Host example outcome |
| --- | --- | --- |
| OpenCode | 1.18.25 | Two completed turns, recalled phrase, third run cancelled, 15 records reopened |
| Codex | CLI 0.153.4, codex-acp 1.10.0 | Two completed turns, recalled phrase, third run cancelled, 19 records reopened |
| Claude | — | Authenticated checks deferred; not verified through this host |

Both checks used `host/example.ts`, disposable SQLite files, and a disposable workspace.
History reopened through a fresh host without creating another provider session.
Record counts differ with provider output; they are observations, not API expectations.

An initial Codex attempt recalled the phrase twice and failed the demo's exact-output
assertion. The demo now checks phrase inclusion, which is the intended context test.
The repeated text reinforces the need for typed message boundaries in H1. The repeated
check completed all steps. Cancellation may lose to normal completion; both observed
checks here ended cancelled.

At that initial commit, storage-lock isolation, slow-reader shutdown, forced-crash
recovery, host application tools, cross-platform lifecycle, and package installation
were unverified. The follow-up evidence below updates that position. See the
[quality gates](../docs/quality-gates.md).

## Review-driven fault tests

The follow-up adds eleven host tests with 107 assertions. The 175 Rust tests, Clippy,
default-feature-free compilation, rustfmt, and TypeScript checks also pass locally.
The tests now force two distinct streams to interleave, verify each session recalls
its own value, and compare exact saved records and paginated reads after restart.
Permission checks reject foreign tokens, stale run IDs, invalid options, and duplicate
responses while proving the legitimate requests still complete.

An external SQLite transaction reproduced the old five-second lock wait. The host
now uses a 100 ms busy-handler budget. The test requires an explicit recording error
and unknown outcome within two seconds while another database's session can run and
cancel. It verifies no completed record is fabricated and no prompt is replayed when
the lock is released. Filesystem and mutex stalls remain outside this evidence.

A Rust consumer stops reading the host pipe without background JS stream buffering.
Linux tests check stdin EOF, stdout disconnect, a full pipe followed by EOF, and a
finite output burst with stdin kept open. The last two hung with the old writer.
All now exit with provider descendants stopped within five seconds. The finite-burst
case exercises the one-second output deadline without filling the output queue.
Output failure ends the whole host and may lose queued events.

The stricter live demo requires successful completed turns, no recording errors, one
terminal event, recall of a fresh UUID-bearing phrase, an actual cancellation attempt,
and exact transcript equality after restart. Only explicit terminal-race errors may
be tolerated by cancellation. Visible agent text is compared separately from reasoning;
only the cancelled run may retain interrupted records. Earlier attempts exposed those
two incorrect assumptions in the demo's assertions, which were corrected to match the
record model rather than dropping the checks.

Both stricter live checks passed on September 7, 2026 with the same provider versions
listed above. OpenCode completed two turns, cancelled the third, and exactly reopened
15 records. Codex did the same with 20 records. These are observed outcomes, not fixed
record-count expectations or a guarantee that cancellation always wins the race.

Forced host death, change subscriptions, shared-database contention policy, arbitrary
disk stalls, host application tools, and non-Linux lifecycle remain open.

## State synchronization and test cleanup, September 8

178 Rust tests, fourteen host tests with 128 assertions, Clippy, rustfmt,
default-feature-free compilation, and TypeScript checking passed locally.
Schema 7 tests cover coalesced old-record updates, overlapping snapshot pages,
database/session cursor validation, future positions, migration backfill, and rollback.
The client test saves a projection/cursor pair, verifies a failed refresh leaves it
unchanged, and resumes through a fresh host without duplicate transcript items.

OpenCode 1.18.25 and Codex CLI 0.153.4 through codex-acp 1.10.0 passed the stricter
live demo with typed projections and saved checkpoints. Both completed two turns
and cancelled the third; exact reopened state contained 13 and 20 records respectively.

The Codex test enabled `delete_session_on_close`. The ACP deletion request succeeded,
and a read-only check of the exact test thread in Codex's database confirmed
`archived = 1`. The pinned adapter maps deletion to archive, not permanent erasure.
Four older exact host-smoke matches were also archived through Codex's supported
app-server API after validating their workspace and first test prompt.

The fixture suite covers successful provider cleanup with bridge records retained,
and cleanup rejection causing a nonzero host exit. These checks do not establish
cleanup after abrupt host death or failure before the adapter returns a session ID.

## Shared-database readers, September 8

180 Rust tests, seventeen host tests with 290 assertions, Clippy, rustfmt,
default-feature-free compilation, and TypeScript checking passed locally.
This increment used deterministic fixtures and did not create provider-history threads.

A new regression test first failed against the previous host: reading history under
an external reserved writer lock returned `Busy` because opening a reader also ran a
migration transaction. Host read endpoints now open an existing current-schema database
read-only. They read committed state under a reserved lock; an exclusive lock fails a
refresh without advancing its checkpoint, and ping still responds within the deadline.

Two integration cases run three sessions sharing a database while independent
projections refresh. They verify distinct session output, terminal outcomes, exact
history and restored projections, and preservation of the application's DELETE or
WAL journal mode. Rust tests verify read-only connections reject mutations, never
create missing databases or migrate older schemas, and see committed updates from
independent writers.

This policy bounds SQLite lock waits and reports failures. It does not establish
arbitrary disk-stall isolation, independent slow subscribers, or writer fairness
under sustained load. No provider-specific behavior changed in this increment.

## Storage worker deadlines, September 8

185 Rust tests, including five new worker-fault tests, and seventeen Bun host tests
with 290 assertions passed. Clippy, rustfmt, default-feature-free compilation, and
TypeScript checking also passed. This increment used fixtures, not live Codex threads.

The controlled blocking-store test holds an agent-message write beyond the 500 ms
caller deadline. The run reports a storage timeout, its handle is retired, cancellation
reaches the ACP fixture, and the provider plus its descendant exit before storage is
released. Another storage worker remains usable. Releasing the blocked operation
afterward produces the late record, without a fabricated completion or prompt replay.

Additional tests cover a blocked read retaining its budget slot, blocked initialization
rejecting replacement workers at capacity, storage panic cleanup, and discarding work
that timed out while waiting for its database writer gate.

The initial threading change exposed a shared-WAL start failure under concurrent
writes. Host operations using the same configured database path are now serialized
inside storage workers; both shared DELETE/WAL acceptance tests pass. Read-only
requests remain separate. External writers and path aliases still follow SQLite's
bounded contention policy.

The fault store models a blocked synchronous operation; it does not simulate a kernel
in uninterruptible I/O. No claim is made that threads or in-progress writes can be
force-cancelled. See [runtime isolation](../docs/runtime-isolation.md).

## Independent run observers, September 8

185 Rust tests, twenty-two Bun host tests with 659 assertions, Clippy, rustfmt,
default-feature-free compilation, and TypeScript checking passed locally. The new
checks use fixtures and create no Codex history threads.

The first regression test failed against the previous client: an unread run filled
its event queue, which threw through the shared wire reader and disconnected the
client. Queues now fail only their own subscriber. The fixed test keeps another run
active, verifies it can still cancel, and recovers the unread run's exact saved text.

A gated fixture produces eight events before pausing, proving a small byte limit can
fail independently of the 128-event limit. After release, a fast observer receives
all 160 ordered chunks and one terminal event while slow observers fail locally.
Other tests cover admission limits, idempotent close, pending-iterator cleanup,
late terminal observation, immutable event/permission data, and permission handling
after another observer leaves.

A reattachment test retrieves a live permission missed by a lagged observer, answers
its original token, and completes the same run. It verifies one prompt dispatch and
zero automatic cancellation requests. Shared transport failures remain whole-host
failures; this increment establishes independent observers inside the Bun client.

## Typed receipt readers, September 8

189 Rust tests and twenty-three Bun host tests with 673 assertions passed locally,
along with Clippy, rustfmt, default-feature-free compilation, TypeScript checking,
and a standalone `receipts` feature build without ACP or SQLite.

The readers cover all five existing bridge-owned extension families. Tests exercise
input versions 1–4, restoration versions 1–3, structured-result evidence, the legacy
configuration report, future versions, unknown extensions, and malformed evidence.
Existing ACP fixture workflows now pass their generated receipts through the reader,
including image/policy/skill inputs and accepted/rejected JSON results.

Shared Rust/host vectors verify the transport projection. The host preserves revision
`9007199254740993` exactly as a decimal string, exposes request text without sending
it twice, keeps malformed receipts visible as invalid, and retains unknown payloads.
Legacy client checkpoints rescan to acquire receipt metadata; current checkpoints
and typed views survive reopening the host.

These are interpretation checks, not provider attestation or revalidation of stored
application results. No live Codex threads were created for this increment.

## H1 integration contract, September 8

The complete `rust_integration` example is now compiled and run in the host fixture
suite. Its success case checks generation, change projection, typed receipt reading,
and exact history after reopening. Both success and provider-error cases verify
requested test-session cleanup and owned provider exit. CI builds the example before
running those tests.

189 Rust tests, twenty-five host tests with 684 assertions, Clippy, rustfmt,
TypeScript checking, dependency-free core compilation, and standalone receipt-reader
compilation passed locally. This increment used fixtures and created no live Codex
threads. H1's acceptance scope and remaining boundaries are documented in
[the acceptance audit](../docs/h1-acceptance.md).

## H2 hosted application tools, September 8

192 Rust tests and thirty-five Bun host tests with 755 assertions passed locally.
Clippy, rustfmt, TypeScript checking, dependency-free core compilation, standalone
receipt readers, and standalone dynamic-tool registration also passed.

The fixture agent uses the actual MCP stdio helper and Rust server. Tests verify
filtered discovery, rejected startup calls, stale grants, invalid arguments, spoofed
MCP metadata, cross-binding capabilities, wrong-scope/duplicate/late results, and
independent callback delivery when run observers are closed. Four concurrent calls
produce separately correlated receipts; a fifth is rejected before dispatch.

Failure checks cover callback deadlines, cancellation reaching the application's
signal, ignored late results, rejected provider retries after uncertainty, fresh-binding
requirements, and failed dispatch/return receipt writes. Application input coercion
and non-JSON output cannot become successful tool results. The store queue was raised
from one to eight jobs to support the recorder plus four admitted tool callbacks.

Live checks passed with OpenCode 1.18.25 and Codex CLI 0.153.4 through codex-acp 1.10.0.
Each invoked the Bun handler once and returned its fresh verification token, which
was absent from the prompt and declaration. Exact reopened history contained nine
records for OpenCode and thirteen for Codex. Codex session cleanup succeeded, and a
read-only check of the exact test thread confirmed `archived = 1`.

The existing Rust-example PID check was changed from immediate PID disappearance to
the bounded stopped-process check used elsewhere. It still fails if the provider
remains running; exited Linux zombies are treated as stopped while awaiting OS reaping.

These results establish the first hosted-tool workflow, not complete H2. Hosted
questions, selected context, validated agent results, dynamic binding changes, other
platforms, and post-crash reconciliation remain open.
