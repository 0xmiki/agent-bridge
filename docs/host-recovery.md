# Hosted restart and restoration

H3 provides read-only discovery after restart and two explicit restoration choices.
It does not restart jobs, replay tools, resend decisions, or turn unknown outcomes
into failures that are safe to retry. Linux fixture tests cover host process death;
this is not a power-loss or distributed-execution guarantee.

## Discover saved work

```ts
const host = new BridgeHost(hostBinary);
const sessions = await host.discover(database, "sessions");
const runs = await host.discover(database, "runs");
const interactions = await host.discover(database, "interactions");
const continuations = await host.discover(database, "continuations");
```

No session or run IDs from the previous process are needed. Each response contains
`items`, `next_after`, `page_full`, and `automatic_replay: false`. While `page_full`
is true, pass `next_after` as the third argument to read the next page. The fourth
argument is a limit from 1 to 1000, defaulting to 100. A full last page may be followed
by an empty page. IDs sort lexicographically, not by time.

These are separate read-only queries against an existing, current-schema database.
They neither launch a provider nor mutate saved state. Stop previous writers before
scanning: pages do not share one snapshot, and this API does not prove that another
host or an application effect has stopped. The existing storage-worker deadlines
and `history_failed` errors apply. No cursor advances on a failed request. Large
histories may exceed the deadline; reduce the page size. Run summaries read each
run's records, and tool-outcome matching currently scans session receipts.

The Rust equivalents are `SqliteStore::discover_sessions`, `discover_runs`,
`discover_interactions`, and `discover_continuations`.

Run discovery includes completed runs as well as unresolved ones:

| Field | What it establishes |
| --- | --- |
| `dispatch: "not_dispatched"` | A preparation marker exists but no dispatch intent was saved. The current recorded path does not send without that intent. |
| `dispatch: "attempted"` | Intent was saved before sending. The provider might or might not have received it. |
| `dispatch: "missing_evidence"` | There is no recognized dispatch marker, including for older runs. Do not infer that nothing ran. |
| `completion` | A stored provider stop reason, or `null` when none was recorded. |
| `issues` | Missing completion, open records, a recorded failure, or missing result validation. |

A completion reason is not proof of successful tools, accepted permissions, or a
valid result. Inspect interactions separately, including when a run has no issues.
Unknown or malformed dispatch receipt versions fail discovery rather than implying
that dispatch was safe. Raw history remains available for inspection.

Interaction discovery returns record references, not executable jobs. It includes
questions without stored answers, permissions without stored decisions, permission
responses whose provider delivery is unacknowledged, incomplete provider tools, and
application tool attempts without a recorded returned result. Permission responses
remain unacknowledged even when the local transport accepted them. Application tool
records are session/slot scoped, not attributed to an invented parent run. Questions
retain their invocation source. Read the session history for full inputs and answers.

`pendingQuestions()` and `pendingPermissions()` still describe live responders.
After restart, saved questions cannot resume the old invocation. A stored answer
does not establish that its enclosing tool completed. A returned tool receipt records
the application's reported result, not the provider's consumption of it.

## Choose the next action

Inspect uncertain effects in their destination system first. The application can
leave the work untouched, resume an available native handoff, or create a fresh
native session with explicitly selected portable context. None of these choices
rewrites or resolves the old uncertain evidence. Any new task needs a new run ID;
the host generates one. External reconciliation and idempotency belong to the
application that owns the effect.

### Native handoff and resume

Create the original session with an application-owned `continuation_scope`, such
as an account/profile namespace. Do not put credentials in that string.

```ts
const session = await host.createSession({...launch, continuation_scope: "profile-a"});
// Finish and drain work before handing off.
const saved = await session.handoff();
await host.close();

const nextHost = new BridgeHost(hostBinary);
const resumed = await nextHost.restoreSession(
  {...launch, continuation_scope: "profile-a"},
  {strategy: "native", session_id: session.id, continuation: saved.continuation_id},
);
const run = resumed.run("Continue with this explicit new task");
```

`handoff()` releases the Bun session handle. Do not call it during an active run
or on a session configured for deletion. The client conservatively retires its
handle on every handoff attempt, including errors. Existing run handles still allow
cancellation. A persisted handoff can be found through continuation discovery even
if its acknowledgment was lost.

The underlying Rust continuation contract checks provider resume support, adapter,
profile scope, provider name/version, and saved workspace. Session and slot identities
are retained. Preflight rejection leaves the continuation available. Claiming is
atomic and single-use; failure or death after claim leaves it claimed. There is no
automatic claim release, fallback to `session/new`, or prompt replay. Native context
is `reused_uninspected`: the bridge verifies the resume protocol, not hidden state.
An interrupted run cannot be handed off as if it were quiescent.

Tools must be granted explicitly again. Reusing a tool scope requires a new host,
even after a failed restore. Each host retains up to 128 used-scope tombstones so
delayed callbacks cannot acquire replacement grants. Portable restoration gets a
fresh slot. The session command queue closes before a handoff or setup-error reply
allows another restore attempt.

### Portable selection

```ts
const restored = await host.restoreSession(launch, {
  strategy: "portable",
  session_id: savedSessionId,
  context: {mode: "append_to_native", records: selectedMessageIds},
});
const run = restored.run("Use this selection for a new task");
```

The application session must already exist. The host validates same-session message
selections and explicit text resources using the H2 limits, freezes them, and creates
a new native session and slot. It does not transfer unselected history, hidden native
state, configuration, old tool grants, or skill activation. `restoration` on the
returned session is a setup report, not delivery evidence.

For both restoration strategies, the first recorded run must have no context or
result options. That turn records the restoration report; portable restoration also
records delivery of its frozen selection. Failed preparation does not consume the
selection. After dispatch, later turns support the normal H2 options and do not
automatically replay the selection. Failure after dispatch retains the existing
retired-session rules. Closing an undispatched restored session can still request
provider cleanup when `delete_session_on_close` is set.

`restore_session` and `handoff` are distinct wire commands. Older hosts reject them
rather than silently creating a fresh session. Saved records use existing extension
payloads; H3 introduces no database migration. Core unrecorded `start_run` calls do
not acquire durable execution evidence through this change.

## Verification scope

Linux tests send SIGKILL after dispatch, while permission input is pending, after
sending a permission decision, during an application effect and a tool-scoped
question, after provider completion while SQLite prevents recording it, and after
durably recorded completion. Reopened hosts discover the evidence without prior
IDs and send no replacement prompts or callbacks. Storage-trigger tests verify that
failed dispatch/decision intent writes prevent sending. Separate native and portable
fixture tests cover single-use claims, scope mismatches, no fallback, exact selected
wire input, one-time delivery, and stale client handles. The packaged consumer also
checks discovery and native handoff/resume through installed package imports.

These checks do not establish recovery after power loss, cross-process exclusion,
automatic reconciliation, or native resume support for every installed provider.
Prior live Rust restoration evidence is recorded in [restoration.md](restoration.md);
new hosted H3 behavior is verified with deterministic providers, not a new live-provider
compatibility claim.
