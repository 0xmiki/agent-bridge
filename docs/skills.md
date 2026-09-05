# Versioned skill inputs

A `SkillRequest` references an immutable resource revision and an explicit delivery
choice. Add requests to `ContextPolicy.skills`; an empty list preserves existing
behavior. A skill document can come from an application archive or a snapshot of a
`SKILL.md` file. The bridge does not require a particular directory layout.

The current contract covers a UTF-8 plain-text or Markdown document, not automatic
installation of an entire skill bundle. Supporting files must be selected separately
as resources. Scripts, dependencies, frontmatter interpretation, and provider-native
installation remain outside this input contract.

## Delivery choices

| Choice | Behavior |
| --- | --- |
| `RequireNative` | The current ACP driver rejects it before prompt dispatch, or before new-session setup during portable restoration. It cannot verify native registration/activation of the exact requested revision. |
| `SupplementalText` | Explicitly deliver the document as supplemental user-level guidance. This is a portable fallback, not a native skill invocation. |
| `Omit { reason }` | Exclude the document without resolving it and retain the nonblank reason. |

No automatic fallback occurs after a native requirement fails. A provider may have
its own installed skills; these choices describe what this bridge can establish
for the requested resource revision, not a blanket statement about that provider.

```rust,ignore
let instruction = InstructionRef {
    resource: skill_revision.clone(),
    role: InstructionRole::Supplemental,
};
let mut policy = ContextPolicy::for_host(host_actor.clone(), vec![instruction]);
policy.skills.push(SkillRequest {
    resource: skill_revision,
    delivery: SkillDelivery::SupplementalText,
});
// Pass policy in ContextTask or PortableRestore.
```

Every requested skill, including an omitted one, requires an exact supplemental
instruction grant for its resource revision. Requester/issuer checks remain those
of [instruction authority](context-policy.md). A changed revision needs a matching
grant. Skill selection does not grant tools or execute scripts; existing provider
tool configuration remains separate.

Duplicate skill references and blank omission reasons fail. If an omitted skill's
resource remains selected through another input, preparation fails. An explicitly
omitted instruction cannot be silently reintroduced by a skill-text request for the
same instruction reference. Selection limits include skill requests, and resolved
document bytes use the existing resource limit.

## Availability and activation evidence

Input receipt data version `4` adds a `skills` array alongside the existing context,
authority, omission, and wire evidence. Each entry preserves its resource revision
and these distinct facts:

- `planned_delivery`: supplemental text or omission.
- `local_availability`: `resolved` for a loaded document, `not_checked` for omission.
- `native_availability`: `unknown` for the exact requested revision.
- `native_activation`: `not_observed` by the bridge for that revision.

Successful text delivery or a correct answer does not change native activation
evidence. Nor does a matching tool title or mention of a skill name establish that
the provider activated this exact document revision. Native provider events remain
available through the existing event/record path without invented correlations.

Skill text uses the same resource allocation and instruction encoding as other
supplemental input. The effective run manifest contains those instruction references;
the original manifest and skill intent remain in receipt evidence. No file is copied
into a provider's configuration directory. Earlier receipts retain their own revision
and bytes when a later run selects a new version.

Portable restoration retains the chosen skill policy and uses restoration-report
data version `3`. Its first run supplies the selected fallback; later turns do not
replay it. Native skill activation state is not transferred. Use
`RestorationPolicy::portable(plan)` to construct a portable policy; the enum stores
the growing plan behind a `Box`.

Policy-free receipt versions remain unchanged. SQL schema is still version 4, and
outer record JSON is still version 1. New native integrations will need actual
capability and observation mappings before they can claim registration or activation.

## Verification

```sh
AGENT_BRIDGE_SKILL_FALLBACK=1 \
cargo run --features acp,sqlite --example acp_context -- \
  /tmp/skill-context.sqlite3 /absolute/disposable-workspace opencode acp
```

The example treats its two answer-style documents as versioned skill inputs. The
second revision requests an uppercase answer. It checks the answer changes and
reopens skill, authority, omission, and delivery evidence from SQLite. The flag is
an example option, not a library-wide setting.

Verified September 6, 2026 with OpenCode 1.18.25 and Codex ACP 1.10.0 using local
Codex 0.153.4. Both passed. Native activation was not claimed. Tests also cover
ungranted revisions, explicit omission of unavailable documents, conflicting
selections, native-requirement rejection, and portable restoration. Claude remains
deferred under the agreed provider-verification scope.
