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

## H1 — Host integration proof (complete within documented scope)

Outcome: launch agents, run and cancel work, and build correct local state without
implementing ACP parsing, routing, or process supervision. The direct Rust path
exposes typed adapter events; the Bun client uses host DTOs. Acceptance:
[quality gates 1–3](docs/quality-gates.md) within the [recorded H1 scope](docs/h1-acceptance.md).

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
- [x] Typed readers for the five bridge-owned receipt families, version handling, precision, and checkpoint upgrades.
- [x] Shared-database contention policy: separate read-only queries, DELETE/WAL concurrency tests, bounded explicit lock failures.
- [x] Bounded storage workers: controlled stall, cancellation/cleanup, late-write uncertainty, capacity, and panic tests.
- [x] Independent Bun run observers: bounded queues, scoped lag, unsubscribe, and live permission recovery.
- [x] Document hosted/direct Rust usage and error/ownership boundaries; compile and fixture-test the complete Rust example.

H1 acceptance, checks, and retained limitations are recorded in
[h1-acceptance.md](docs/h1-acceptance.md). This completes the integration proof; it
does not release an alpha or claim equivalent runtime guarantees for every embedding
path. H2 below tracks hosted interactions. Tools and tool-scoped questions are
implemented; context and result handling are the remaining work.

Test hygiene: disposable Codex checks must request provider session cleanup after
verification. `host/example.ts` enables this automatically for codex-acp launches;
custom wrappers must set `AGENT_BRIDGE_CODEX_TEST=1`. The pinned adapter archives the
thread out of active history. Cleanup failure must be reported, not silently ignored.

## H2 — Application interactions (in progress)

Outcome: tools and questions use the same integration. Acceptance: quality gate 4.

- [x] Hosted application tools with runtime schemas and live application handlers.
- [x] Derive MCP bindings from grants; reject stale revisions, wrong capabilities, and cross-scope results.
- [x] Tool cancellation, bounded calls, once-only result acceptance, and typed receipts; unknown outcomes retire the binding.
- [x] Real hosted tool invocation with OpenCode and Codex, including Codex test-thread cleanup.
- [x] Tool-scoped hosted structured questions and atomic answers, with cancellation, pending-form recovery, and live OpenCode/Codex checks.
- [ ] Hosted selected-context delivery and validated agent results.

Use existing Rust primitives. Add child APIs when an acceptance scenario establishes
application ownership; a general multi-agent scheduler is not required.
The first tool workflow is documented in [host-tools.md](docs/host-tools.md). It uses
Unix IPC with Linux verification and a bounded runtime-schema subset.
[Hosted questions](docs/host-questions.md) reuse the same invocation ownership and
record validated answers. Next slice: selected-context delivery and validated agent
results through the host. Standalone hosted questions remain an extension to consider
when an acceptance scenario needs them.
This does not complete H2 or the post-crash recovery contract.

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
locally tested host platform. Bun observer isolation and controlled storage-stall
handling are implemented. Shared-transport fairness, actual OS fault evidence,
hosted interactions, and crash recovery remain outside H1's completed scope.

Defer full Tauri packaging, all three app migrations, remote stores, non-ACP drivers,
native internal subagent control, general routing/scheduling, and broad native skill/output
parity. A small Tauri probe is appropriate for a concrete lifecycle question.

The independent pre-host review rated architecture 7/10 and roadmap 6/10. A defensible
9/10 needs integration and fault guarantees; 10/10 needs sustained consumer and release
evidence. No milestone or test count automatically earns a score.
