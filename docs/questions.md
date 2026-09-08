# Structured questions and answers

The `records` feature now includes `Payload::Question` and `Payload::Answer`.
They are distinct from permission requests and decisions. A valid answer is input
data, not an execution grant or proof that a provider received it.

Questions contain a title and up to 64 uniquely named fields. Fields support bounded
text, Boolean values, bounded integers, and a single selection from up to 256 named
options. Required means a value must be present; `false` is a valid required Boolean
answer. Choice IDs are validated independently of their display labels.

`AnswerOutcome` distinguishes submitted values, decline, and cancellation. Submitted
values cannot include unknown fields, use the wrong type, omit required fields, or
exceed declared bounds. Decline and cancellation do not require filled fields. Text
limits count UTF-8 bytes. This is a small form model, not arbitrary JSON Schema.

## Atomic storage

Insert a question as an open record. Its definition is immutable while pending.
Use `RecordStore::resolve` to append an answer and finalize the question atomically.
Direct answer insertion and direct completion of an unanswered question are rejected.
A permission decision cannot resolve a question, and an answer cannot resolve a
permission request.

An answer must reference the original question and retain its session and optional
run attribution. Invalid answers leave the question open. Repeating an identical
resolution is idempotent; a conflicting second answer fails. The same rules apply
to memory and SQLite, using the existing resolution table and transaction boundary.

`AnswerDelivery` distinguishes `Stored`, `Queued`, and `Unknown`. The local helper
below always records `Stored`. Saving or returning an answer locally does not
establish transport delivery. Final answer records are immutable; later transport
evidence must not rewrite the original answer.

## Awaitable application questions

Enable `questions` for `PendingQuestion` and `QuestionResponder`.

```rust,ignore
let pending = PendingQuestion::open(store.clone(), question_draft, host_actor)?;
let responder = pending.responder();
// Route responder to the application's UI or another host component.
let answer = pending.wait(cancellation_token).await?;
```

The question is persisted before the responder is exposed. A host component calls
`responder.answer(answering_actor, outcome)`. Validation and persistence happen before
the waiting operation receives the answer. Invalid submissions may be corrected
without losing the pending question. Cloned responders share one resolution gate.
The original definition snapshot is available through `responder.question()`;
read the store for current resolution state.

Cancellation resolves an unanswered question with a stored cancellation attributed
to the host. If a submitted answer already won, cancellation returns that answer
instead of overwriting it. Dropping the waiter attempts the same cancellation on a
best-effort basis. Store failures during explicit operations are returned; drop
cannot report a write failure and may leave a question open.

These handles are process-local. Use unique question IDs and resolve live waits
through their responder. `open` rejects an already-existing ID, but this is not a
distributed exclusive waiter claim. Two independently created hosts are not an
answer-delivery coordination system. Reopening SQLite restores records, not waiting
futures, provider requests, or tool execution. Open questions after a crash require
application recovery policy; they are not automatically replayed.

The host controls who receives the responder and authenticates the answering actor.
The actor ID is attribution, not a credential. Storage methods are synchronous and
can block, so schedule them appropriately for UI and async hosts. Questions do not
modify `ToolGrant` values, install instructions, or authorize downstream effects.

## Application-tool composition

`application_questions` registers a granted asynchronous tool that creates a
question, passes its responder through a host channel, and waits for the answer.
It passes the invocation's cancellation token into the wait. A deterministic host
task selects an option, and the handler resumes with that validated value.

```sh
cargo run --features tools,questions,sqlite --example application_questions -- \
  /tmp/questions.sqlite3
```

This is scripted test input, not a real user's approval. The example verifies that
the resumed handler returns the selected value and that the question and answer
survive SQLite reopen. It runs without a provider or network connection.

This increment supports application-managed questions. It does not advertise or
map native ACP elicitation, URL forms, or provider-specific question tools. Native
question transport requires a separate mapping with reliable session attribution;
unverified native support is not implied by these record types. The
[host/client question channel](host-questions.md) now composes these records with
granted application-tool invocations and Bun UI callbacks.

## Format compatibility and verification

SQLite schema version 5 introduced the gate for these payloads. New JSON documents use
format version 2; the decoder retains format-1 support and rejects newer unknown
versions. Migration preserves existing rows without rewriting their documents.
Pre-question libraries reject the newer schema instead of writing to a database containing types
they cannot interpret. See [SQLite storage](sqlite.md).

Tests cover field validation, wrong response kinds, atomic/idempotent resolution,
competing answers, cancellation/drop, failed answer persistence, legacy upgrade,
and reopen. The scripted application-tool example passed on September 7, 2026.
