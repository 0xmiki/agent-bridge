# Working milestones

Updated September 8, 2026. Revise this plan when evidence warrants it and record why.
The previous M0–M8 plan and evidence remain in [milestone history](milestone-history.md).

## Product promise

Embed installed ACP agents with a typed application API, owned process lifecycle,
reliable local history, and application tools. The “better-auth of ACP” ambition is
adoption without rebuilding protocol plumbing, with extension points for application
context, tools, storage, and presentation.

The [slot philosophy](philosophy.md) remains. The independent review found integration
and recovery evidence weaker than the core model, so delivery now follows application
features. The [host](host/README.md) is our acceptance application. Existing Rust
features are not automatically completed host features.

## H1 — Host integration proof (in progress)

Outcome: launch agents, run and cancel work, and build correct local state without ACP
objects in application code. Acceptance: [quality gates 1–3](docs/quality-gates.md).

- [x] Rust host and versioned stdio protocol with Bun client.
- [x] Fixtures: concurrent sessions, repeat turns, permissions, cancellation,
  active-process close, history reopen, malformed input, and unsupported versions.
- [x] Add Linux CI for Rust checks and host tests/typechecking.
- [x] Record real OpenCode and Codex host checks ([evidence](host/verification.md)).
- [x] Failed-startup recovery test.
- [x] Linux EOF/disconnect/stalled-reader and descendant cleanup tests with deadlines.
- [x] Distinct interleaved streams, retained fixture context, and exact reopened history.
- [x] Reject foreign/stale/invalid/duplicate permission responses without consuming valid requests.
- [x] Explicit SQLite lock failure, independent-database session progress, and no automatic retry.
- [x] SQLite snapshot/change cursors, old-record updates, and initial typed client projections.
- [ ] Extend typed readers to bridge-owned receipts and settle the public projection API.
- [x] Shared-database contention policy: separate read-only queries, DELETE/WAL concurrency tests, bounded explicit lock failures.
- [x] Bounded storage workers: controlled stall, cancellation/cleanup, late-write uncertainty, capacity, and panic tests.
- [x] Independent Bun run observers: bounded queues, scoped lag, unsubscribe, and live permission recovery.
- [ ] Document equivalent Rust usage and the settled error/ownership contract.

Next slice: typed readers for bridge-owned receipts, then the Rust integration and
error/ownership documentation needed to finish H1. Observer isolation is scoped to
the Bun client; a failed shared transport still fails the host. The host has an outer storage wait
budget, separate from SQLite lock waits, with explicit uncertainty and bounded worker
capacity. [Controlled fault tests](docs/runtime-isolation.md) cover stalled operations;
actual kernel I/O failure and OS termination evidence remain release work. This is a
bounded failure contract, not a promise that an in-progress write can be cancelled.
The first [state synchronization contract](docs/state-sync.md) is implemented with
coalesced record changes rather than a transcript-revision log. Fault tests exposed and
fixed unbounded stdout writes, long lock waits, and colliding permission tokens. The
new bounded failure policies do not replace the remaining storage/recovery work.

Test hygiene: disposable Codex checks must request provider session cleanup after
verification. `host/example.ts` enables this automatically for codex-acp launches;
custom wrappers must set `AGENT_BRIDGE_CODEX_TEST=1`. The pinned adapter archives the
thread out of active history. Cleanup failure must be reported, not silently ignored.

## H2 — Application interactions (planned)

Outcome: tools and questions use the same integration. Acceptance: quality gate 4.

- [ ] Host/client tools, questions, selected context, and validated results.
- [ ] Mechanically bind tools from grants; reject stale and cross-session input.
- [ ] Handler cancellation, bounded active work, and atomic duplicate decisions.
- [ ] Real application tool invocation through the host per supported provider.

Use existing Rust primitives. Add child APIs when an acceptance scenario establishes
application ownership; a general multi-agent scheduler is not required.

## H3 — Local restart contract (planned)

Outcome: discover and explain unfinished work after a crash. Acceptance: quality gate 5.

- [ ] Enumerate unresolved runs/interactions without in-memory IDs.
- [ ] Persist dispatch intent/outcomes and expose explicit recovery choices.
- [ ] Kill/reopen tests at dispatch, tool, decision, and completion boundaries.
- [ ] Host APIs for verified native resume and explicit portable restoration.
- [ ] Never automatically replay effects with uncertain outcomes.

Reading saved history alone does not establish durable execution.

## H4 — Public ACP alpha (planned)

Outcome: an independent developer installs packages and succeeds. Acceptance: gate 6.

- [ ] Package Rust and TypeScript entry points; compile consumer examples.
- [ ] Previous-database upgrade fixtures and wire compatibility checks.
- [ ] Publish provider/version/OS evidence and limitations.
- [ ] Lifecycle and descendant cleanup on every advertised OS.
- [ ] Independent consumer integration/review; resolve release blockers.

The current crate version is scaffold metadata, not a released alpha.

## Scope and evidence

OpenCode and Codex have prior real Rust evidence; host results are tracked separately.
Claude authenticated checks remain deferred at the user's request. Linux is the only
locally tested host platform. Subscriptions, storage stalls, and recovery remain open.

Defer full Tauri packaging, all three app migrations, remote stores, non-ACP drivers,
native internal subagent control, general routing/scheduling, and broad native skill/output
parity. A small Tauri probe is appropriate for a concrete lifecycle question.

The independent pre-host review rated architecture 7/10 and roadmap 6/10. A defensible
9/10 needs integration and fault guarantees; 10/10 needs sustained consumer and release
evidence. No milestone or test count automatically earns a score.
