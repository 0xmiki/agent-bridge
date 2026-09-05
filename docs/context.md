# Context preparation

This is the first M3 increment. Enable `records` to use `agent_bridge::context`.
The contract is provisional and does not yet dispatch prepared inputs to providers.

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

Next we need capability-aware delivery and durable receipts of what was supplied.
Provider-native context, portable history restoration, instruction authority,
images, skills, and structured-result validation remain distinct M3 concerns.
Preparation alone must never be recorded as successful delivery.
