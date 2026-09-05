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
selection and prepare again. [Context policies](context-policy.md) add exact,
authorized omissions through a separate policy-aware preparation path.

The limits bound selection count and unique resource bytes. They are not token
budgets, record-payload size limits, or pre-fetch memory limits for custom stores.
A resource store must bound its own I/O and allocation. Prepared values retain
shared snapshots in memory; preparation itself does not write a database record.

## Recorded ACP text delivery

`AcpSession::start_recorded_context_run(id, task, store, actors)` prepares the
manifest internally against the destination session's history and the task's
resource store. `ContextTask` includes the task prompt, manifest, resource store,
preparation limits, maximum encoded prompt bytes, and an explicit
`TextContextMode::AppendToNative` choice.
It also requires `policy`; the default denies declared instructions. Use an exact
instruction grant as described in [instruction authority](context-policy.md).

This mode sends one JSON text envelope containing selected conversation messages,
their authors and revisions, supplemental instruction references, unique resource
texts, and the new task. It uses ACP's mandatory text input capability. Historical
speaker labels remain data in that text; they are not reconstructed native message
roles. Native session context remains in place. Nothing automatically removes
already-known history or makes a portable clone of hidden provider state.

The text encoder accepts user/agent messages and UTF-8 plain-text or Markdown
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
`agent_bridge`, name `input_receipt`, and an inner data version. Version 1 covers
policy-free text, version 2 adds images, and version 3 adds policy evidence described
in [context policies](context-policy.md). Consumers must check all three identifiers.
Subsequent receipts refer to the same run through its
record attribution; only the preparation receipt includes the wire text.

| State | Established fact |
| --- | --- |
| `prepared` | Encoded input evidence, encoding version, byte count, context mode, and omission report were persisted before dispatch. |
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
snapshot per run. Text receipts retain this representation for compatibility;
image receipts below use the separately retained resource bytes. The receipt describes text
prepared for the provider, not its hidden context. Existing text-only run methods
retain their previous behavior and do not create these context receipts.

Try the real example with an existing absolute workspace:

```sh
cargo run --features acp,sqlite --example acp_context -- \
  /tmp/context-example.sqlite3 /absolute/disposable-workspace opencode acp
```

It selects a stored message, grants two supplemental instruction revisions across
two turns, verifies the changed answer style, and reopens its receipts from SQLite.
The second turn records an explicit optional-resource omission. It uses the configured model,
dismisses tool permission requests, and has a 60-second workflow timeout.

Verified September 5, 2026 with OpenCode 1.18.25 and Codex ACP 1.10.0 using local
Codex 0.153.4. Both returned the selected history's exact codename and preserved
the three receipt states after SQLite reopen. Fixture tests additionally compare
the receipt text to the actual protocol request, preserve historical attribution,
reject unsupported base instructions and oversized prompts before dispatch, and
exercise receipt-write failure, process crash, and dropped-run behavior.

## Durable resources and image delivery

`ResourceArchive` adds an optional write contract to `ResourceStore`. The in-memory
archive remains available, and `SqliteStore` now implements both traits. Its schema
version 4 retains exact revisions and shares identical blobs by SHA-256 digest.
See [resource retention](sqlite.md#resource-retention).

Set `ContextTask.mode` to `ContextMode::AppendImagesToNative` to permit images in
addition to text. `ContextMode::AppendToNative` preserves the previous text-only
behavior; `TextContextMode` remains an alias for existing callers. The image path
requires the provider's advertised ACP image capability. It accepts exact media
types `image/png`, `image/jpeg`, `image/gif`, and `image/webp`, with matching file
signatures. Full decoding, dimension checks, and model-specific acceptance remain
with the provider. Base instructions still cannot use this path.

The first prompt block is the text envelope. Each unique selected image revision
gets one subsequent ACP image block with base64 data. References in selected
messages resolve through the same resource map as direct selections. The envelope
associates each image reference with its block index, media type, original byte
count, and SHA-256 digest. The bridge does not fetch URLs or substitute captions.

Image prompts use encoding `agent_bridge.media_context.v1` and input receipt data
version `2`. The preparation receipt retains the text envelope and image descriptors,
not base64 image copies. Later delivery-state receipts use version 2 for that run.
Without instruction authority or omissions, text-only prompts use receipt version 1;
policy evidence uses version 3 for either text or images. The outer record JSON format
is unchanged. For image inputs, `wire_bytes` and the prompt limit cover the serialized
ACP content-block array, including base64 and escaping; they exclude request IDs and
other RPC framing. Oversized images fail before dispatch.

Image receipts depend on the supplied resource store for retained bytes. Using an
in-memory store does not make them durable. Use `SqliteStore` or an application archive
and retain those revisions for as long as receipts need to be inspectable. A reader
can resolve the recorded reference and compare its bytes with the recorded digest;
the [image example](../examples/acp_image.rs) does this after SQLite reopen.
No base64 copies are added to each transcript receipt, though sending an image to
a provider still requires encoding and transmitting it for that run.

```sh
OPENCODE_CONFIG_CONTENT='{"model":"opencode/mimo-v2.5-free"}' \
cargo run --features acp,sqlite --example acp_image -- \
  /tmp/image-example.sqlite3 /absolute/disposable-workspace \
  /path/to/solid-red.png red opencode acp
```

This verification example accepts a PNG and an expected color. The expected answer
is used only for local validation, not included in the prompt. It reopens the image
store before dispatch and verifies the receipt digest after another reopen. It uses
the configured model, dismisses tool permission requests, and times out after 60 seconds.

Verified September 5, 2026 with a generated 32-by-32 solid-red PNG:

- Codex ACP 1.10.0, local Codex 0.153.4, reported `gpt-6-astra`: passed.
- OpenCode 1.18.25, reported `opencode/mimo-v2.5-free`: passed with an explicit
  process-local model configuration.
- OpenCode's default `opencode/big-pickle`: did not identify the image and returned
  a statement that it could not view images. The workflow check failed despite a
  normal prompt response. No automatic model fallback occurred.

Protocol image support is necessary but not proof of model-level success. The
example validates the answer independently of input-delivery evidence. Claude
verification remains deferred. [Restoration policy](restoration.md) now makes native
resume and portable selection explicit. [Instruction grants and omission policies](context-policy.md)
now govern declared instruction changes and excluded selections. Native base
replacement remains unsupported; skills and structured-result validation remain M3 work.
