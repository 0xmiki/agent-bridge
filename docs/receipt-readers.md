# Typed receipt readers

The optional `receipts` feature exposes `agent_bridge::records::receipts::read`.
It depends on portable records and the shared structured-result types, without
requiring ACP or SQLite. The host enables it and includes receipt metadata in
history, snapshot, and change responses.

```rust
use agent_bridge::records::receipts::{read, Receipt, InputStage};

if let Some(Receipt::Input(receipt)) = read(&snapshot.record.payload)? {
    match receipt.stage {
        InputStage::Prepared(input) => show_input(&input.wire_text),
        InputStage::ResponseReceived { stop_reason } => show_response(&stop_reason),
        InputStage::DispatchAttempted => show_attempt(),
        InputStage::Unknown => show_uncertainty(),
    }
}
```

Use `read` for version and consistency checks; directly deserializing an individual
data struct does not perform those checks. The caller retains the original record,
including its actor, run ID, and raw extension payload.

| Extension in `agent_bridge` | Supported inner format | Reader |
| --- | --- | --- |
| `input_receipt` | Versions 1–4 | Preparation, delivery stages, images, instruction policy, skill evidence |
| `restoration` | Versions 1–3 | Native-resume or portable-selection report |
| `result_contract` | Version 1 | Requested contract and host validation settings |
| `result_validation` | Version 1 | Valid/rejected decision with source revisions and rejection detail |
| `configuration_report` | Existing unversioned format | Optional provider-reported configuration values |

Unknown namespaces or names return `None`; the extension remains available through
the original payload. Future versions return `Receipt::Unsupported`. Missing or
malformed known-format evidence returns `ReceiptError`. Unrecognized top-level
receipt fields remain in the original payload and are ignored by the typed view.
Nested domain types retain their own validation rules.

Readers check required fields, identifiers, numeric types, input byte consistency,
image descriptor shape, policy/skill version requirements, and contradictory result
claims. They do not inspect retained image bytes or verify grants against current
authority. A legacy version-3 restoration report may lack policy detail; that absence
is preserved rather than reconstructed.

## Host and TypeScript

```ts
import { readReceipt } from "../host/receipts";

const receipt = readReceipt(record);
if (receipt?.kind === "result_validation") {
  renderValidation(receipt.data.validation, receipt.data.sources);
}
```

`SessionState.items` also exposes `{ kind: "receipt", receipt, record }` items.
The Rust host validates receipt data once. The TypeScript reader uses that metadata;
invalid known receipts become explicit `invalid` views and future versions become
`unsupported` views. Neither condition destroys the rest of a transcript.

Counters and record revisions in the host view are decimal strings, so values above
JavaScript's safe-integer range stay exact. Inner format versions remain numbers.
Request text is sent once in the original payload; the TypeScript reader exposes
`wire_text` from there. Unknown raw JSON is not interpreted by the receipt reader.

Projection checkpoints now carry `projection_version: 1`. Unversioned checkpoints
trigger a new scan to obtain receipt metadata, even for records whose revisions have
not changed. Future checkpoint versions fail explicitly. No agent is launched by
that scan, and no database migration or receipt rewrite is required.

## Meaning and evidence

Preparation is not dispatch. Dispatch is not a response. Restoration setup is not
context delivery. Host result validation is not native output enforcement. Recorded
skill text is not evidence of native skill activation, and a configuration report
is not proof of which model served every internal step.

The readers preserve these distinctions; they do not synthesize run-wide success,
replay work, reconstruct application validators, or attest to the author of a record.
These are provisional reader contracts that can grow with actual provider evidence.

Tests cover all current input/restoration versions, malformed and future formats,
real receipt producers in the ACP fixture suite, and shared Rust/host vectors. Host
checks include exact large revisions, request-text deduplication, invalid-view handling,
and checkpoint upgrade/reopen behavior.
