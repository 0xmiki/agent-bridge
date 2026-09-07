# Execution relationships and child authority

The `records` feature exposes `ChildPlan`, `ChildRequest`, `DelegationGrant`, and
`ExecutionStore`. A parent/child edge describes bridge-managed delegation between
registered runs. It does not start work, grant permissions by itself, or represent
every causal relationship in a conversation.

## Plan an explicit child

`ChildPlan::new(store, parent_authority, delegation_grant, child_request)` loads the
registered parent and checks the requested assignment. The host supplies both the
parent's tool grant and a separate delegation grant. Permission to use a tool alone
does not imply permission to delegate it.

`DelegationGrant` identifies the issuer, permitted delegating actor, parent run,
allowed context references, and allowed application-tool revisions. `ChildRequest`
selects a new run ID, slot, actor, exact context, and tool revisions. The application
session stays the same; cross-session import is not implicit.

The plan requires:

- Matching host issuer and delegating subject between grants.
- A parent tool scope matching the registered parent's session and slot.
- Child tools contained in both the parent tool grant and delegation grant.
- Selected records, resources, and instructions contained in the delegation grant.
- A different run ID and no child native-continuation claim.

Nothing copies the parent's full context or tool list by default. An empty child
selection stays empty. Grants authorize selected references, not arbitrary prompt
text or a provider's private/default context. The host authenticates identities and
authorizes reference visibility before constructing these objects.

When the parent is itself a recorded child, its parent-authority argument must equal
its recorded child tool grant. A caller cannot invent a broader grant for further
delegation within that run. Context access still requires an explicit host-issued
delegation grant; the bridge does not infer visibility from conversation membership.

## Dispatch through ACP

Create a fresh native session with the plan's application session and child slot,
then call `start_recorded_child_run(&plan, task, store, actors)`. The task's effective
context after policy processing must equal the planned selection. Existing context
authorization, resource, and capability checks still apply. The recorded host and
agent must match the child grant's issuer and subject.

The method rejects reused or resumed native sessions. This prevents prior
bridge-dispatched conversation from silently becoming child context. Initial provider
defaults still apply. Model configuration is selected on the child session and frozen
for its run; it is not copied from the parent.

The child run is registered and its execution relation is saved before prompt
dispatch. A failed relation write prevents dispatch, though the run ID may already
be registered and cannot be reused. Input receipts and subsequent output recording
use the existing recorded-context path. A stored edge is not proof of dispatch,
completion, or a currently active parent.

`plan.authority()` returns the narrowed application-tool grant. Bind the child's
application MCP server to that grant and its subject/session/slot. The child-run
method does not inspect or configure arbitrary MCP endpoints for you.

These grants govern this bridge's registered application handlers. They do not
sandbox provider-native tools, inherited account access, filesystem access, or
provider-internal subagents. Configure native provider controls separately. An empty
application-tool grant is not a claim that the provider has no built-in tools.

## Store contract

Memory and SQLite implement `ExecutionStore`:

- `link_execution` requires both registered runs and validates the grant snapshots.
- A child has at most one immutable parent edge. Identical repeats are idempotent;
  conflicting edges fail.
- Self-links and cycles fail. Adding a parent to an existing subtree cannot change
  the tool authority under which its recorded children were admitted.
- `execution_parent` and `execution_children` query recorded relationships. Child
  results are ordered by ID, not execution time. They are not active-job counts.

The relation retains parent authority, the delegation grant, and child authority.
Selected context and actual configuration remain in the child run spec. Both runs
share an application session, but may have different slots and output actors.
All selected record references must belong to that session.

SQLite schema 6 adds `agent_bridge_execution_relations`, with foreign keys to both
runs and a parent lookup index. Writes are transactional, including cycle and
authority checks. Existing records and document formats are unchanged: writers use
format 2 and readers retain format-1 support. This remains local bookkeeping, not
distributed dispatch, automatic cascading cancellation, or crash reconciliation.

## Example and verification

```sh
cargo run --features acp,sqlite --example acp_child -- \
  /tmp/children.sqlite3 /absolute/disposable-workspace \
  /absolute/path/to/opencode acp
```

The application runs a parent turn, selects a recorded input, explicitly grants that
context to a child with no application tools, and creates a fresh child session.
It checks that the child returns the selected phrase and reopens the parent edge
and tool grant from SQLite. The application orchestrates the child; it is not a
claim that the parent model autonomously spawned it. The example dismisses native
permission requests and has a 120-second workflow timeout.

Verified September 7, 2026 with OpenCode 1.18.25 and Codex ACP 1.10.0 using local
Codex 0.153.4. Both passed. Tests cover authority/context narrowing, inherited-grant
consistency, cycles, immutable edges, reopen, identity corruption, fresh-session
requirements, and failure before dispatch. Provider-managed subagent observations,
concurrency/delegation limits, and multi-participant routing remain M4 work.
