# Instruction authority and omissions

`ContextTask` and `PortableRestore` now require a `ContextPolicy`. Its default is
strict: no omissions and no authority to supply declared instructions. Use that
default for selections containing only history and resources.
Versioned [skill requests](skills.md) use `ContextPolicy.skills` and the same exact
instruction-grant boundary. Their delivery and activation evidence remain separate.

`prepare` remains a provider-independent resolution primitive. Use
`prepare_with_policy` when inspecting a policy-controlled selection. Recorded ACP
context runs and portable restoration use this policy path internally.

## Exact instruction grants

`InstructionGrant` names an issuer, a subject, and exact `InstructionRef` values.
Each reference includes resource ID, revision, and intended role.
`InstructionAuthorization` identifies the requester presenting that grant.

The bridge checks that the requester matches the subject and every requested
instruction is included in the grant. At dispatch, the issuer must match
`RecordActors.host`. Changing a revision or role requires a matching grant. The
grant also covers instructions explicitly omitted from the requested selection;
omission cannot bypass the authorization check.

For a host-owned selection:

```rust,ignore
let policy = ContextPolicy::for_host(host_actor.clone(), manifest.instructions.clone());
// Pass policy in ContextTask and use the same host_actor in RecordActors.host.
```

For a delegated proposal, the host constructs an `InstructionAuthorization` whose
requester and grant subject identify the proposing agent or participant. The issuer
remains the host. A model can propose a resource revision, and application policy
can approve it by creating a matching grant. Appending a proposal to history never
creates a grant or dispatches work by itself.

These are trusted host decisions, not signed credentials or a caller authentication
system. Authenticate external requesters before constructing grants. Do not accept
an alleged host grant from model output as authorization. The library checks declared
instruction references, not the semantics of arbitrary conversation text. Grants
do not authorize tools, file writes, model configuration changes, or delegation.

## What changes mean

The supported ACP mechanism delivers supplemental instructions as explicit user-level
text for one run. Supplying revision v2 after v1 records a new input and authority
decision without rewriting the earlier receipt. It does not delete v1 from retained
native history or attest that the provider obeyed the new guidance.

An included `Base` instruction still fails the ACP encoder even with a valid grant.
Authorization and provider capability are separate requirements. There is no silent
conversion of a base instruction into supplemental text. A host may explicitly omit
that exact instruction with a matching grant, which records that it was not supplied.
This does not remove or alter the provider's own native instructions.

The currently supported instruction-change contract is revision selection and
supplemental delivery. Native base replacement remains unsupported until a provider
mechanism can establish it. Use the restoration policy when choosing whether to
retain native conversation state; omission is not a history-erasure operation.

## Explicit omissions

`ContextOmission` names a `ContextItem` and a nonblank reason. Items can be an exact
record ID, instruction reference, or direct resource reference from the requested
manifest. No wildcard or automatic "skip missing" policy exists.

```rust,ignore
policy.omissions.push(ContextOmission {
    item: ContextItem::Resource(optional_reference),
    reason: "Not needed for this assignment".into(),
});
```

Omissions are applied before resolution. A specifically omitted input need not exist
in its store. Unknown omission targets, repeated omission entries, blank reasons,
and ungranted instruction omissions fail. One omission removes every duplicate
selection of that exact item. Requested selection count, omission count, and grant
scope count remain bounded by `ContextLimits.max_items`; omission is not a way to
bypass that count limit. The resource byte limit applies to retained resources.

If an omitted direct resource is still referenced by a retained message or
instruction, preparation fails before fetching it. Omit the dependent selection
explicitly or revise the request. The bridge never partially rewrites a record.

This is selection-level exclusion, not content redaction. Omitting an instruction
does not hide the same bytes if the host separately selected them as ordinary
resource content. Nor does any omission erase information already present in native
context. The caller is responsible for constructing a selection appropriate to its
application's data-access rules. Resource stores retain their own access boundaries.

## Persistent evidence

`PreparedContext.requested_manifest` keeps the original selection. Its `manifest`
contains the effective selection after omissions; that effective manifest is what
the run registers. This avoids creating durable run references to unavailable
omitted records. The prepared context keeps its policy as immutable shared grant
data and owned omission entries.

Runs with instruction authorization or omissions use input receipt data version
`3`. The preparation receipt adds `requested_context`, `instruction_authority`,
and exact omission items/reasons to the existing wire evidence. The authority field
records issuer, requester, subject, and granted instruction revisions/roles. These
audit fields are local records, not additional grant-bearing text sent to the model.
Later delivery receipts use the same version for that run.

Portable restoration with either policy feature uses restoration-report data version
`2`, with a `context_policy` field. Policy-free reports and native restoration retain
version 1. Policy-free input receipts retain their prior text/image versions 1/2.
The outer record JSON format stays at version 1 and SQL schema stays at version 4.
Readers must inspect namespace, record name, and inner data version.

Missing authorization or unsupported retained inputs fail before prompt dispatch.
Writing required input/restoration evidence still precedes dispatch. A grant records
host permission, and a response receipt records provider response evidence; neither
is proof of native instruction replacement or correct task output.

## Verification

The updated `acp_context` example uses instruction v1 for the original codename,
then grants v2 for an uppercase response and explicitly excludes an unavailable
optional resource. Both turns' instruction references and the omission reason
survive SQLite reopen. It passed on September 5, 2026 with OpenCode 1.18.25 and
Codex ACP 1.10.0 using local Codex 0.153.4.

Tests cover wrong requester/issuer, ungranted revision/role changes, immutable prior
receipts, exact missing-input omissions, duplicate/unknown omissions, dependency
conflicts, and restoration-report preservation. Claude verification remains deferred.
