# Hosted context and results

`session.run(prompt, options)` delivers selected context, validates a JSON result,
or does both. Streaming, cancellation, permissions, tools, and questions still work.
Option-bearing runs use the v1 `run_task` command so older hosts reject the request
with `unknown_method` instead of silently ignoring context or validation. Upgrade
the host and client together; plain `run(prompt)` keeps its existing command.

```ts
const run = session.run("Summarize the selected work", {
  context: {
    mode: "append_to_native",
    records: [selectedMessage.id],
    resources: [{
      id: "brief", revision: "v2", media_type: "text/markdown",
      text: "The application-selected project brief.",
    }],
  },
  result: {
    name: "summary", revision: "v1", mode: "validate_returned_text",
    max_validation_bytes: 4096,
    schema: {
      type: "object", properties: { summary: { type: "string" } },
      required: ["summary"], additionalProperties: false,
    },
  },
});
for await (const event of run.events) {
  if (event.event === "text_delta") showDraft(event.text);
  if (event.event === "permission") await run.respond(event.permission_id, null);
}
const finish = await run.completed;
if (finish.result?.status === "valid") useValidatedJson(finish.result.value);
```

The UI functions above belong to the application. See the complete
[runnable example](../host/interactions-example.ts) and the
[packaged consumer](../verification/consumer/README.md), which also uses a tool and question.

## Selected context

Record IDs must resolve to final user or agent messages in the current bridge
session and database. Missing, open, foreign-session, reasoning, and non-message
records fail before dispatch. Selected snapshots are rechecked before recording.
There is no cross-session import or implicit history selection.

Resources are explicit UTF-8 strings with application-assigned IDs and revisions.
Only `text/plain` and `text/markdown` are supported. The per-run memory store rejects
conflicting bytes at the same revision within that run. Applications own revision
identity across runs; this is not a durable resource catalog. The exact encoded
text is retained in the input receipt independently of that temporary store.

`append_to_native` adds a user-level text envelope. It does not erase native context,
replace system instructions, activate skills, or prove the model used every item.
Images, instruction grants, omissions, and restoration are not hosted by this API.
Unsupported modes and fields fail explicitly.

Limits: 128 selections including message attachments, 256 KiB of unique resource
bytes, and 512 KiB of encoded context prompt. The 1 MiB request-frame limit also
applies. These are byte limits, not token budgets. Preparation and dispatch attempts
have separate saved receipts; they are not a crash-safe execution contract.

## Validated results

The host compiles the bounded local JSON-schema subset used by
[hosted tools](host-tools.md). Schemas are limited to 64 KiB, 256 nodes, and depth 16.
Remote references and unsupported assertions fail before dispatch. Validation limits
must be 1 through 65,536 bytes. The recorded contract carries an application name,
revision, and provider instructions containing the schema.

The provider must finish normally and return exactly one JSON value in one assistant
message. Streaming chunks are joined. There is no fence removal, extraction, coercion,
repair, or automatic retry. Multiple messages, non-text output, invalid JSON, schema
violations, oversized output, and incomplete turns produce explicit rejections.
Draft text is never a validated value.

`run.completed` reports provider/recording status and a separate `result`:

- `valid` includes a host-validated JSON `value`, typed as `unknown` for the application
  to narrow. It does not promise an arbitrary TypeScript type or business-rule check.
- `rejected` includes a typed rejection. A completed provider run can have a rejected
  result. Cancellation is incomplete even if the text parses.
- `unavailable` means no trustworthy result can be exposed, including failed recording.
- `null` means validation was not requested. Older hosts may omit this field.

Contracts are saved before dispatch. Validation receipts reference source message
revisions and must be saved before a valid value is exposed. Failed recording reports
an error and an unavailable result. Read evidence through `readReceipt`; reopening
history does not replay validation or effects. The byte limit bounds validation, not
the existing transcript buffer.

Native schema enforcement is unsupported. A schema checks structure and values,
not the truth or safety of the model's answer.

## Evidence

Linux fixtures cover combined and context-only runs, exact wire text, reopened
receipts, invalid selection/schema rejection before dispatch, result rejection,
cancellation, and failed input/contract/validation writes. The packaged Bun consumer
checks context and results alongside tools and questions.

On September 12, 2026, OpenCode 1.18.25 and Codex CLI 0.154.0 with codex-acp 1.10.0
passed the live example. Both returned a fresh token supplied only as selected text,
then accepted selected history in a second validated turn. Native history remained
present; this does not prove history replacement or restoration. Exact reopened
history contained 20 OpenCode records and 23 Codex records. Codex reported successful
test-session cleanup. Claude and non-Linux checks remain deferred.
