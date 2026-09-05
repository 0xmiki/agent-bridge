# Context preparation

Enable `records` to use `agent_bridge::context`. Preparation remains independent
of providers. With `acp`, recorded text-context delivery is also available below.
These M3 contracts remain provisional.

`ContextManifest` selects history, instruction revisions, and resources. `prepare`
resolves those selections into a `PreparedContext`. It keeps the manifest's record
order, author attribution, record revision and finalization state. Interrupted
records stay marked interrupted. Open records are rejected because their content
can still change.

The host supplies permitted session IDs and a resource store with the appropriate
access scope. This allows explicitly selected history from multiple sessions
without treating every session as shared. The session list is a host-provided
constraint, not an authentication system. Existing run registration still requires
same-session context records; cross-session dispatch needs an explicit import or
delivery contract in a later increment.

`ResourceStore::get` must return the exact immutable revision. The included
`MemoryResourceStore` accepts identical repeated writes and rejects conflicting
content or media types at the same revision. New content needs a new revision.
Revision labels are application-assigned identifiers, not content hashes. A custom
store owns access control, retention, and storage errors.

Preparation resolves resource references inside selected messages as well as direct
resource selections and instructions. Repeated resource references share an `Arc`
allocation and count once against the unique resource byte limit. Records and
instructions retain their requested ordering and duplicates. Resource enumeration
uses first-reference order: message attachments, instructions, then direct resources.
Opaque extension payloads are preserved without searching their JSON for resources.

Instructions remain separate from conversation history. Their bytes must be UTF-8
`text/plain` or `text/markdown`; the intended `Base` or `Supplemental` role survives
preparation. Resolving a base instruction does not establish provider support or
grant authority to change that provider's instructions.

```rust,ignore
use agent_bridge::context::{prepare, ContextLimits};

let prepared = prepare(
    &manifest,
    &record_store,
    &resource_store,
    &permitted_sessions,
    ContextLimits { max_items: 100, max_resource_bytes: 4 * 1024 * 1024 },
)?;
```

Missing inputs, scope violations, open records, invalid instruction content, and
exceeded limits fail preparation. Nothing is automatically dropped, summarized,
replaced with a newer revision, or sent to a provider. A caller can make a different
selection and prepare again; explicit omission policies remain future work.

The limits bound selection count and unique resource bytes. They are not token
budgets, record-payload size limits, or pre-fetch memory limits for custom stores.
A resource store must bound its own I/O and allocation. Prepared values retain
shared snapshots in memory; there is no new database schema in this increment.

## Recorded ACP text delivery

`AcpSession::start_recorded_context_run(id, task, store, actors)` prepares the
manifest internally against the destination session's history and the task's
resource store. `ContextTask` includes the task prompt, manifest, resource store,
preparation limits, maximum encoded prompt bytes, and an explicit
`TextContextMode::AppendToNative` choice.

This mode sends one JSON text envelope containing selected conversation messages,
their authors and revisions, supplemental instruction references, unique resource
texts, and the new task. It uses ACP's mandatory text input capability. Historical
speaker labels remain data in that text; they are not reconstructed native message
roles. Native session context remains in place. Nothing automatically removes
already-known history or makes a portable clone of hidden provider state.

The current encoder accepts user/agent messages and UTF-8 plain-text or Markdown
resources. Base instructions, reasoning records, non-message activity records, and
binary resources fail before run registration or dispatch. Supplemental instructions
are explicitly delivered as user-level guidance. The bridge does not silently
claim system authority or infer instruction support from image/context capabilities.
Unsupported inputs require a different selection or a future delivery mechanism.
No input is silently omitted.

Run registration retains the selected manifest and the existing frozen model
configuration. The user message record contains the task as entered; a separate
host-attributed receipt contains the exact encoded wire text. The encoded-byte
limit includes JSON escaping and metadata. It bounds output size, not all temporary
memory allocated while constructing the envelope.

## Input receipt evidence

The recorder writes immutable `Payload::Extension` records with namespace
`agent_bridge`, name `input_receipt`, and data version `1`. Consumers must check
all three identifiers. Subsequent receipts refer to the same run through its
record attribution; only the preparation receipt includes the wire text.

| State | Established fact |
| --- | --- |
| `prepared` | Exact encoded text, encoding version, byte count, context mode, and the empty omission list were persisted before dispatch. |
| `dispatch_attempted` | The bridge persisted dispatch intent immediately before calling the transport. The provider may not have received it. |
| `response_received` | A correlated prompt response was observed; its native stop reason is included. This does not prove every input was consumed or obeyed. |
| `unknown` | The run failed or recording ended before such a response was persisted. No retry is implied. |

Store errors while writing either pre-dispatch receipt prevent dispatch. Registration
may already exist, so that run ID cannot be reused. A crash after dispatch intent
but before the transport call is indistinguishable from later uncertain dispatch
without additional evidence. After a host crash, the last persisted state can remain
`dispatch_attempted`; the library does not invent a later `unknown` record on reopen.
Drop-time recording remains best effort. Storage and provider execution are not
one transaction; M5 owns broader reconciliation.

The exact text snapshot makes the receipt inspectable after reopening SQLite even
if the in-memory resource store is gone. It deliberately costs one bounded text
snapshot per run. Future durable resource retention can reduce that duplication;
references alone would not preserve these bytes today. The receipt describes text
prepared for the provider, not its hidden context. Existing text-only run methods
retain their previous behavior and do not create these context receipts.

Try the real example with an existing absolute workspace:

```sh
cargo run --features acp,sqlite --example acp_context -- \
  /tmp/context-example.sqlite3 /absolute/disposable-workspace opencode acp
```

It selects a stored message and a versioned supplemental instruction, verifies the
answer, and reopens its input receipts from SQLite. It uses the configured model,
dismisses tool permission requests, and has a 60-second workflow timeout.

Verified September 5, 2026 with OpenCode 1.18.25 and Codex ACP 1.10.0 using local
Codex 0.153.4. Both returned the selected history's exact codename and preserved
the three receipt states after SQLite reopen. Fixture tests additionally compare
the receipt text to the actual protocol request, preserve historical attribution,
reject unsupported base instructions and oversized prompts before dispatch, and
exercise receipt-write failure, process crash, and dropped-run behavior.

Provider-native restoration policy, base-instruction authority, durable resources,
images, explicit omission policies, skills, and structured-result validation remain
M3 work. Preparation alone is never evidence of successful delivery.
