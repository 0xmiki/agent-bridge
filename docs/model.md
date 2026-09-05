# Working model

Status: draft, September 2026. This is a starting point for implementation, not
a frozen API or database schema. Change it when examples or provider behavior
show a better boundary. See [philosophy.md](../philosophy.md) for the intent.

Implementation checkpoints: the core types and lifecycle now have tests; the
optional [ACP adapter](acp.md) can create sessions and stream text runs using the
shared lifecycle. [Recorded runs](records.md) assemble portable payloads through
a provisional local store backed by memory or [SQLite](sqlite.md). SQLite has a
version 1 persistence format; future changes must migrate it or retain its decoder.
Durable execution and provider continuation remain separate, open contracts.

## Four concepts

| Concept | Owns | Does not imply |
| --- | --- | --- |
| Slot | A stable identity for configured execution capacity | A persona, a model, or a permanently running process |
| Session | A durable identity for related records and work | One provider session or an automatically shared context |
| Run | One execution assignment and its lifecycle | A chat message or a whole workflow |
| Record | An attributed piece of input, output, or activity | An instruction to execute more work |

Slots can serve different sessions. A session can use different slots across
runs. Model selection belongs to each run; changing defaults must not rewrite
the configuration of earlier runs. Requested and provider-confirmed settings
must remain distinguishable.

A record identifies its author separately from its producing run. An application
can call a participant "Reviewer" while choosing different executors for its
work. The core does not need to own participant profiles.

## History, context, and continuation

History is the application's record of what happened. Reading it must not require
launching an agent. Context is the explicit information supplied for one run;
it need not include every record in the session.

A context manifest references selected records, versioned instructions, and
resources. Instructions retain their intended role. An adapter must not silently
substitute ordinary prompt text for a required base-instruction mechanism.
Referenced record content must be immutable or revision-addressed once used by
a run. Summaries are derived records; they do not replace source history.

Provider continuation is separate. It can reuse a native session that already
contains context, avoiding full transcript replay on every turn. A resume handle
does not promise a snapshot or transferable private state. We need compatibility
and synchronization checks before reusing it, especially after another slot has
contributed to the session.

Model changes normally apply between runs. Within-provider changes may preserve
native continuation. Cross-provider changes require an explicitly allowed context
transfer. The visible session identity stays the same in either case. The runtime
must report omissions or unsupported requirements before execution.

## Lifecycle rules

The first implementation models a bridge-managed run as:

```text
queued -> starting -> running -> completed | failed | cancelled
              |          |
              +----------+-> cancelling -> cancelled | completed | failed
              |          |
              +----------+-> unknown -> running | completed | failed | cancelled
```

- Cancelling queued work can settle locally, before it is dispatched.
- Dispatch moves the run to starting before external I/O. A lost acknowledgement
  may mean the provider already accepted work; it must not leave the run queued.
  Providers may report a terminal outcome without a separate start notification.
- Requesting cancellation of running work records intent, not success. Completion
  may win the race. A lost connection while starting, running, or cancelling yields
  unknown. A late start acknowledgement must not erase cancellation intent.
- Unknown means we cannot establish the execution outcome. Reconciliation requires
  provider evidence; silence does not prove failure or make retry safe. A recovered
  running execution with a pending cancellation returns to cancelling.
- Terminal states do not reopen. A retry is a new run with its own identity.
- Waiting for permissions or questions is represented by interactions. It is
  separate from the run's execution lifecycle because several requests may coexist.

The initial state machine is an in-memory model. It does not dispatch processes,
resolve provider evidence, enforce budgets, or make concurrent writes atomic.
Those responsibilities belong to the runtime and storage implementations.

## Records and resources

Records have typed IDs, session-local ordering, optional run attribution, and
optional response relationships. Message content can reference a resource rather
than copying its bytes. Streaming updates build an in-progress record; we do not
need a durable row for every token.

The `records` feature now defines message, tool activity, permission, decision,
completion, failure, and extension payloads. Open records accept revision-checked
checkpoints; finalized records are immutable. SQLite now versions its tables and
serialized payloads. Resource storage, an extension registry, and future migration
steps remain open.

Permission records preserve offered options and local delivery state. The memory
store accepts one valid decision atomically with request finalization. General
question schemas, permission scope enforcement, and durable delivery still need
contracts.
Tool declarations describe available operations; grants authorize execution.
Neither ordinary metadata nor instruction text grants authority.

## Composition examples

- Chat: append user input, select context, start a run, and display its records.
- Background work: start a run with document resources and an output requirement;
  no chat view is necessary.
- Group conversation: an application routes records to runs attributed to selected
  participants. Appending a record alone never triggers another run.
- Delegation: a child run references its parent and receives explicit context and
  authority. Provider-managed children may be observable without being independently
  controllable. The first state machine covers bridge-managed runs only.

## Persistence direction

Start with slots, sessions, runs, and records, plus provider continuation storage
when we implement native resume. Resources can use a separate store. This is a
logical model, not a requirement to squeeze everything into four SQL tables.

Memory and SQLite implementations now obey the same tested behavioral contract:
stable IDs, unique session ordering, guarded state changes, atomic interaction
resolution, and duplicate-write handling. A stored run is not a distributed job
queue. Worker claims and leases need a separate contract if we add that capability.

## Questions to settle through implementation

- Which instruction, history-restoration, model-switching, and structured-output
  semantics do the released ACP adapters actually preserve?
- How should context manifests describe native context we cannot inspect?
- What continuation state is needed to detect missing or duplicated context?
- How should run configuration expose provider-specific options without leaking
  transport details into every application?
- Which lifecycle facts can we observe for native subagents, and how do we represent
  partial visibility without inventing state?
- What minimal storage operations preserve these rules across memory and SQL?

Implement one complete execution path before expanding the API to answer every
question. Keep useful counterexamples and revise this document alongside the code.
