# Hosted structured questions

H2 now connects application-tool questions to the Bun application's UI. This reuses
the Rust `PendingQuestion`/`QuestionResponder` model; it does not advertise native ACP
elicitation or translate provider-specific question tools.

```ts
const lookup = defineTool({
  name: "project_lookup",
  revision: "v1",
  description: "Choose a project action",
  input: lookupInput,
  async execute(input, context) {
    const answer = await context.ask({
      title: "Choose an action",
      fields: [{
        id: "action", label: "Action", required: true,
        kind: { type: "select", data: { options: [
          { id: "review", label: "Review" },
          { id: "skip", label: "Skip" },
        ] } },
      }],
    });
    return answer;
  },
});

const host = new BridgeHost(binary, {
  tools: [lookup],
  onQuestion: async question => {
    const values = await showForm(question.definition, question.signal);
    await question.answer({ type: "submitted", data: values });
  },
});
```

The application supplies `showForm`; no UI framework is imposed. Values are tagged
as text, Boolean, integer, or selected option IDs. See [questions.md](questions.md)
for the field and answer model. `submitted`, `declined`, and `cancelled` are data
outcomes, not execution grants. A required Boolean may validly be false.

## Ownership and persistence

`context.ask` is available only to an active, granted application-tool invocation.
The host derives its session/slot and source invocation from the live call. A caller
cannot select a different store or scope through question arguments. Question records
are attributed to `host`; answers supplied by the application UI are attributed to
`user`. These are local participant labels, not proof of human identity or approval.

The host persists the open question before emitting `question_opened`. It validates
the answer against that immutable definition and revision, then atomically stores
the answer and finalizes the question before releasing the waiting tool. Invalid
values or scope/revision mismatches leave the valid question usable. Identical answers
are idempotent while the invocation remains live; conflicting answers fail. After
the invocation ends, its live response handles are stale.

Question records link to their invocation through `source.namespace =
agent_bridge.tool_invocation` and `source.id`. Answer records link to the question
through `reply_to_id`. Both retain null `run_id`, matching the invocation's explicit
lack of attested native-run attribution. Host history now exposes those existing
source/reply fields. No new SQL schema or document format is required.

`AnswerDelivery` remains `stored`: neither persistence nor returning to the application
proves that the provider consumed the answer. Tool-return receipts capture the later
application result separately. Cancellation can race a stored answer; it must not
overwrite an answer that already won the storage gate. A stored answer does not resume
an ended invocation. Cancellation still cannot undo effects or preempt application code.

## UI recovery and closure

`onQuestion` enables question callback protocol 1. Alternatively, use `questions: true`
and `await host.pendingQuestions(sessionId)` to render pending forms manually. A UI
callback exception is available as `QuestionHandle.error`; it does not silently answer
or decline the form. Pending-query results can be used to recover it.

`QuestionHandle.signal` closes when the question closes, its tool is cancelled, or the
client stops. It is a UI lifecycle signal, not the answer itself. Delayed pending-query
responses cannot resurrect already closed forms. Answer through the handle rather than
reconstructing live authority from saved history.

Run observers are independent of this channel. Closing `run.events` does not discard
a question. Ending the owner invocation cancels outstanding questions and releases
live responders. Drop-time recording is best effort; a storage timeout can leave an
open question or a late stored answer requiring application review. [Restart
discovery](host-recovery.md) exposes saved evidence but does not revive responders.

## Bounds and deadlines

- One pending question per invocation, at most eight questions during that invocation,
  and at most 128 live/cached responder entries across the host.
- Thirty-two admitted question operations, two question-runtime workers, and sixteen
  concurrent UI callbacks. A UI callback that ignores closure retains its callback slot.
- Definitions are limited to 16 KiB; submitted answer JSON to 60,000 bytes. The core's
  64-field/256-option limits remain. Hosted text fields allow at most 16 KiB, and integer
  bounds/values must be JavaScript-safe integers. Rust's direct question model is wider.

Questions share the owning tool's wall-clock deadline; asking does not reset it.
With questions enabled, the default tool deadline is 120 seconds, otherwise 30 seconds.
`toolTimeoutMs` accepts 50–300,000 ms. Providers can impose shorter limits. Question
wait RPCs outlive that configured deadline by five seconds so the normal 45-second
client request timer does not prematurely kill a valid form wait.

Timeout or cancellation may leave the tool invocation uncertain and retire its MCP
binding. A fresh session is not a guarantee that repeating an external effect is safe.
Closing the host always closes stdin even if a shutdown request cannot be admitted.

## State and verification

`SessionState.items` has typed `question` and `answer` items. Projection checkpoints
now use version 2; unversioned/version-1 checkpoints rescan to acquire source/reply
links. Future versions fail explicitly. A restarted host can read stored questions
and answers, but it does not recreate old waiting futures or live responders.

The runnable [questions-example.ts](../host/questions-example.ts) uses scripted form
input for verification, not real user approval. Tests cover all four field types,
invalid/cross-scope/stale answers, duplicate/conflicting submissions, write failure,
UI failure recovery, cancellation, deadlines, shutdown, admission limits, stale
snapshots, and exact reopened relationships. Native question transport and standalone
host-created forms outside tool invocations remain separate work.
