# Integration quality gates

Working acceptance criteria, September 7, 2026. Revise these when an integration
exposes a better requirement. The independent review rated the pre-host architecture
7/10 and roadmap 6/10. These gates are its proposed route toward a defensible 9/10,
not a promise that a test count produces a score. A 10/10 assessment would need
sustained consumer, release, upgrade, and provider-change evidence.

## 1. Application ownership

- Two fixture sessions run concurrently without crossed events; repeat turns keep context.
- Request IDs, malformed input, unsupported versions, and failed startup have explicit outcomes.
- Cancel, EOF, close, and consumer disconnect clean up owned processes and descendants.
- Each advertised provider completes a real host prompt; consumers implement no ACP parser.

Initial host tests cover concurrent sessions, repeat turns, correlation, cancellation,
permissions, normal active-process close, failed-startup recovery, malformed input,
and version rejection. EOF/disconnect/descendant coverage remains incomplete.

## 2. Application state and storage

- Upsert projections agree with final reopened state.
- Snapshot plus change cursor has no race, missing updates, or duplicated effects.
- Updating an old record is visible after reconnect; slow subscribers have bounded,
  documented behavior.
- Bridge-owned receipts have typed readers; applications do not interpret extension JSON.

History reopen is tested. Creation-sequence pagination is not a change cursor.
Subscriptions, update cursors, and typed client projections remain open.

## 3. Runtime isolation

- Hold a SQLite write lock or stall storage during streaming; another session and
  cancellation must progress within defined deadlines.
- Overflow and recording failures are explicit and do not silently corrupt another session.
- A client that stops reading cannot indefinitely prevent shutdown.

The host uses separate session workers and bounded queues. Core recording is still
synchronous, so isolation and shutdown under these faults are not established.

## 4. Application interactions

- Bind MCP tools mechanically from approved plans and grants.
- Reject spoofed identities, stale revisions, cross-session responses, and expired questions.
- Cancellation reaches application handlers; duplicate decisions have one outcome.
- Exercise a real application tool per supported provider through the host.

Rust tool/question/grant APIs exist. Host permission routing is tested. Host tools,
questions, and automatic binding remain to be implemented.

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
