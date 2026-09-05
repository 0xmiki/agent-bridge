# Structured results

Enable `structured` for typed JSON validation independent of providers. Add `acp`
for recorded structured runs. The core's default features remain unchanged.

`JsonContract<T>` uses `T: serde::de::DeserializeOwned` to validate a returned JSON
value. Its name and revision identify the application's contract; instructions tell
the provider what to return. An optional `with_validation` callback checks domain
rules after deserialization.

```rust,ignore
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskList {
    tasks: Vec<String>,
    count: usize,
}

let contract = JsonContract::<TaskList>::new(
    "task-list", "v1",
    "Return tasks as an array of strings and count as the number of tasks.",
    16 * 1024,
)?
.with_validation(|value| {
    if value.count == value.tasks.len() {
        Ok(())
    } else {
        Err("count does not match tasks".into())
    }
});
let value = contract.validate(returned_text)?;
```

Deserialization accepts exactly one JSON value with optional surrounding whitespace.
It does not extract JSON from prose, remove Markdown fences, coerce types, repair
syntax, or retry generation. Field acceptance follows the chosen Rust type: use
`deny_unknown_fields` when extra fields should fail. A custom deserializer can have
different acceptance rules. Dynamic application validation can use
`JsonContract<serde_json::Value>` with a callback.

The contract's `max_validation_bytes` bounds the candidate text accepted for parsing,
including surrounding whitespace. It is not a token budget, provider generation
limit, or cap on the existing transcript buffer. Validation reads recorded text
without building a second unbounded text accumulator. Callback behavior and contract
revision management belong to the application. Change the revision when changing
the type, validation rules, or expected output contract.

## Recorded ACP runs

`AcpSession::start_recorded_json_run(id, task, store, actors)` accepts a `JsonTask`
containing the prompt, borrowed contract, and explicit `JsonOutputMode`.

- `ValidateReturnedText` asks for JSON and validates the returned text on the host.
- `RequireNativeEnforcement` fails before registration or dispatch because this ACP
  driver does not expose a native structured-output enforcement mechanism.

The bridge sends the task and output instructions as ordinary text. Provider-native
session context still applies. This method does not automatically restore portable
context or install system instructions. Existing context and restoration methods
remain separate operations.

`RecordedJsonRun` exposes events, permissions, cancellation, snapshots, checkpoints,
and the underlying run state. `result()` is `None` until a validation decision is
persisted. After draining, `into_result()` returns the owned typed value or rejection.
Calling it early returns `None` and drops the unfinished recorded run under the
existing cancellation/retirement rules.

A typed value requires a normal ACP `end_turn` response and exactly one complete
agent message. Streamed chunks with the same message identity assemble into that
message. Reasoning text and tool output are not result candidates. Multiple agent
messages are ambiguous and fail; unsupported non-text message content also fails.
The bridge does not guess which message was intended as the final JSON result.

Refusal, token limits, cancellation, and provider/recording errors cannot produce
a valid result, even if the available bytes happen to parse. Parseable JSON received
before the prompt response remains unvalidated. Shape or rule rejection does not
rewrite an otherwise completed provider run to `Failed`: execution and validation
are separate facts.

`JsonRejection` distinguishes missing/ambiguous/non-text output, size limits,
invalid JSON, invalid shape, failed application rules, and incomplete execution.
Host parsing and checks establish only the declared contract, not factual correctness
or provider-native enforcement.

## Persistence

The recorder writes host-attributed extensions in namespace `agent_bridge`:

| Name | Evidence |
| --- | --- |
| `result_contract` | Data version 1, contract name/revision, mode, validation limit, whether an application check exists, and exact request text; persisted before dispatch. |
| `result_validation` | Data version 1, contract name/revision, accepted or rejected status, rejection details when present, and source message IDs/revisions. |

Both records explicitly set `native_enforcement: false`. Output bytes remain in
their existing message records rather than being copied into the validation receipt.
The contract records identify application code but do not serialize the Rust type
or callback. Reading a persisted decision does not reconstruct that code; revalidation
requires the application's matching contract implementation.

A failed contract write prevents dispatch, though the run ID may already be
registered. A failed validation-receipt write returns a recording error and does not
expose the typed value as a persisted success. The completed execution and transcript
can still exist. Dropping or crashing before validation may leave a contract and
partial output without a validation receipt; absence of that receipt is not success.
No automatic repair, revalidation, or provider retry happens on SQLite reopen.

No SQL migration is required. SQL schema remains version 4, and the outer record
JSON document remains version 1. Readers must check namespace, record name, and
inner data version.

## Background-task example

```sh
cargo run --features acp,sqlite,structured --example acp_background -- \
  /tmp/background.sqlite3 /absolute/disposable-workspace opencode acp
```

The example extracts a task list from a short note, deserializes it into a Rust type,
checks that `count` matches a nonempty list, and reopens the validation receipt from
SQLite. It dismisses permission requests and has a 60-second overall workflow timeout.
It is a background data-extraction task and does not require a chat UI.

Verified September 5, 2026 with OpenCode 1.18.25 and Codex ACP 1.10.0 using local
Codex 0.153.4. Both returned three tasks that passed the declared checks, and their
validation evidence survived reopen. Claude verification remains deferred.
