# Implementation milestones

This is the working roadmap for agent-bridge. Read it before choosing the next
implementation task, and update it as work lands. The plan can change when real
provider behavior or application use reveals a better approach. Record the reason
for a change instead of silently replacing the goal.

## Goal

Ship a public alpha that lets developers embed installed AI agents through a shared
runtime, with usable Rust and TypeScript APIs, a Tauri integration, and replaceable
storage. A developer should supply application context, tools, and presentation
without implementing provider protocols, process management, or transcript recovery.

The foundation is the [compute-slot philosophy](philosophy.md) and
[working model](docs/model.md). Your applications are integration tests for that
model. They do not define its boundaries. Chat, background work, and application
tool use must also work in examples independent of those applications.

## Current position

Status checked against `10b9113` on September 5, 2026.

- M0 is complete within the scope below.
- **M1 is the next target. Implementation has not started.**
- The last implementation verification passed 90 tests, Clippy, documentation
  generation, and the core, records, ACP, and SQLite feature builds.
- OpenCode 1.18.25 has passed real prompt streaming and native resume checks.
- Codex and Claude compatibility, TypeScript, Tauri integration, and app migrations
  have not been implemented or verified yet.

The crate's `0.1.0` version is scaffold metadata, not evidence that an alpha has
been released. M8 defines the first release gate.

| Milestone | Outcome | Status | Depends on |
| --- | --- | --- | --- |
| M0 | Execution and persistence foundation | Complete | — |
| M1 | Model selection and attributable run configuration | Next | M0 |
| M2 | Verified multi-provider integration and setup | Planned | M1 |
| M3 | Explicit context, instructions, and rich tasks | Planned | M2 |
| M4 | Application tools, authority, and execution composition | Planned | M3 |
| M5 | Durable execution bookkeeping and bounded recovery | Planned | M4 |
| M6 | TypeScript SDK, subprocess host, and Tauri plugin | Planned | M5 |
| M7 | Real application migrations | Planned | M6 |
| M8 | Public alpha release | Planned | M7 |

This is the default order. Small compatibility probes and CI setup can happen
earlier when they reduce uncertainty. Splitting or reordering a milestone requires
updating this document and its dependencies.

## M0 — Execution and persistence foundation

- [x] Rust crate, Nix development shell, and Conventional Commit workflow.
- [x] Typed identities, slots, sessions, context references, and run lifecycle.
- [x] ACP process initialization, capability reporting, shutdown, and timeouts.
- [x] Session creation, text streaming, tool activity, permissions, and cancellation.
- [x] Portable record assembly with stable identity and coarse checkpoints.
- [x] Memory and SQLite stores with shared validation and contract tests.
- [x] Versioned SQL migrations and serialized records.
- [x] Single-use provider handoff, persisted claims, and explicit ACP native resume.
- [x] Real OpenCode prompt and two-process context-continuity checks.

Evidence: [ACP adapter](src/acp.rs), [records](src/records.rs),
[SQLite adapter](src/records/sqlite.rs), [continuation contract](docs/continuations.md),
and [tests](tests). Commits `a9af0c0` through `10b9113` contain these increments.

Completion here does not mean general crash recovery. Continuations are conservative
single-use handoffs. The runtime cannot yet reconcile an uncertain claim, transfer
context to another provider, or confirm configuration for each run.

## M1 — Model selection and run configuration

Make configuration discoverable and record what an execution requested and what
the provider confirmed.

- [ ] Expose supported session configuration options through a documented API.
- [ ] Validate model and option selections before dispatch.
- [ ] Define requested, confirmed, and unknown configuration states.
- [ ] Persist per-run configuration without rewriting earlier runs or records.
- [ ] Apply model changes between runs, with explicit behavior for unsupported changes.
- [ ] Link runs to the continuation they consume when one is available.
- [ ] Add required migrations and preserve existing persisted data.
- [ ] Verify switching between two available models while retaining a thread's context.

Done when a runnable example changes models between turns, history keeps the same
application session, each run's configuration remains inspectable after reopening,
and invalid or unsupported settings cannot silently disappear. Provider-confirmed
settings must remain distinguishable from unverified defaults.

First implementation slices: configuration discovery, validated selection, then
per-run configuration persistence and the model-switching example.

## M2 — Verified multi-provider integration and setup

- [ ] Define the provider/driver contract above the ACP implementation.
- [ ] Add launch definitions and compatibility checks for OpenCode, Codex, and Claude.
- [ ] Discover installations and distinguish missing CLI, missing adapter, incompatible
  version, setup required, and ready states where these can be established.
- [ ] Support explicit executable paths and provider profile scopes.
- [ ] Provide actionable setup guidance using each provider's supported mechanisms.
- [ ] Run a shared compatibility suite for text, tools, permissions, cancellation,
  configuration, and advertised continuation behavior.
- [ ] Publish tested versions and limitations, separating advertised from verified support.

Done when the same example and application-facing contract work across all three
providers for their declared common capabilities. A provider-specific implementation
is added only for a demonstrated gap; it must satisfy that same contract.

## M3 — Context, instructions, and rich tasks

- [ ] Resolve explicit context manifests from stored records and resources.
- [ ] Record what the bridge supplied, including instruction revisions and omissions.
- [ ] Keep instructions, conversation history, and provider continuation distinct.
- [ ] Support instruction changes with explicit authority and capability requirements.
- [ ] Add resource storage and image input without repeatedly copying large assets.
- [ ] Define explicit native-resume versus portable-context restoration policies.
- [ ] Add structured-result validation; distinguish native enforcement from validation
  of unconstrained output.
- [ ] Represent skills as versioned inputs where supported, distinguishing availability
  from observed activation. Define the fallback policy where native skills are absent.

Done when independent examples cover an interactive conversation with selected
context, an image-based task, and a background task returning validated data. A
provider switch reports the context transferred and anything it could not preserve.

## M4 — Tools, authority, and composition

- [ ] Make existing MCP attachment convenient and verify real app-tool round trips.
- [ ] Add typed application-tool registration and the required MCP bridge.
- [ ] Separate tool declarations, execution grants, and user permission decisions.
- [ ] Preserve structured questions and answers alongside permission interactions.
- [ ] Support execution relationships and explicit child context/authority selection.
- [ ] Represent provider-managed subagent activity without inventing unavailable control.
- [ ] Enforce configured concurrency and delegation limits.
- [ ] Demonstrate application-defined routing between participants and compute slots.

Done when a tool-using assistant and a simple multi-participant example compose the
same primitives without app-specific changes to the core. Tests establish tool
scope, result routing, cancellation, and the limits of child-execution control.
The library need not own a group-chat policy or workflow language.

## M5 — Durable execution bookkeeping and recovery

- [ ] Persist meaningful execution intent and observed lifecycle changes.
- [ ] Define claim, dispatch, acknowledgement, and completion boundaries.
- [ ] Track decision submission separately from confirmed delivery where available.
- [ ] Recover stored application state without automatically repeating provider work.
- [ ] Reconcile native continuation claims only where provider evidence permits it.
- [ ] Preserve an explicit unresolved state and actionable diagnostics otherwise.
- [ ] Test host/process failure around dispatch, permission responses, and completion.

Done when supported recovery paths have reproducible failure tests, unknown outcomes
stay visible, and retries cannot silently duplicate consequential work. This does
not require distributed scheduling or an exactly-once execution guarantee.

## M6 — Application integration packages

- [ ] Establish versioned host/client contracts without exposing ACP messages to apps.
- [ ] Build a typed TypeScript client with incremental record and status subscriptions.
- [ ] Build a subprocess host for Bun/Node applications using the Rust runtime.
- [ ] Build a thin Tauri plugin with lifecycle and shutdown integration.
- [ ] Keep database and process work off frontend/UI threads.
- [ ] Provide examples for Rust, Bun/Node, and Tauri consumers.
- [ ] Document the provider adapter, store, transport, and extension boundaries.

Done when a small Tauri application and a Bun program can use the same TypeScript
API for setup, runs, records, and interactions without implementing protocol routing
or process supervision. Optional framework bindings can follow demonstrated need.

## M7 — Application migrations

- [ ] Inventory current behavior and record migration acceptance cases per app.
- [ ] Migrate Chesscave: position context, coaching configurations, analysis tools,
  streaming, and cancellation.
- [ ] Migrate Experts: existing conversation data, native continuity, persona changes,
  response modes, and historical speaker attribution.
- [ ] Migrate Figmaboy: conversation restoration, references, permissions, and structured
  Evolve jobs with role-specific tool access.
- [ ] Validate an alternative provider for the capabilities each app advertises.
- [ ] Remove replaced integration code after behavioral checks pass.
- [ ] Feed general improvements back into the library and retain domain logic in apps.

Done when the three apps use agent-bridge for their agent infrastructure, existing
data remains usable, and their documented workflows pass. Missing provider features
must be visible rather than presented as equivalent behavior.

## M8 — Public alpha release

- [ ] Establish CI for supported Rust features, SDKs, and package boundaries.
- [ ] Verify process launch, packaging, cleanup, and installation on each declared OS.
- [ ] Provide clean-install examples, setup diagnostics, and a provider compatibility table.
- [ ] Document migrations, limitations, extension points, and the alpha stability policy.
- [ ] Prepare versioned Rust and TypeScript release artifacts and release notes.
- [ ] Check the generic examples and migrated app acceptance cases against release artifacts.
- [ ] Publish the agreed alpha packages.

Done when a developer can follow the documentation to add installed agents to a
new application using released packages. Supported platforms and capabilities must
be backed by checks, and unresolved limitations must be stated accurately.

## How to maintain this plan

1. Before implementation, identify the milestone and the bounded slice being tackled.
2. Mark a milestone in progress when code work starts. Keep only one default next target.
3. Check an item only when code, relevant tests, and documentation support the claim.
   Record evidence in the milestone or link to the implementation commit.
4. Update this file in commits that materially change progress, scope, or dependencies.
   Completing a milestone means satisfying its outcome, not merely adding its types.
5. Revise the plan when evidence warrants it. Note what changed, why, and how it affects
   the release goal. Do not weaken a criterion just to mark work complete.

No calendar dates are promised here. Work progresses through verified increments,
with small `feat:`, `fix:`, `test:`, `docs:`, or `chore:` commits.

## Deferred unless the goal changes

Hosted multi-tenant execution, distributed worker scheduling, automatic installation
of every CLI, a workflow DSL, and a universal chat UI are outside the alpha target.
The extension boundaries should leave room for them without requiring them now.

## Plan changes

- September 5, 2026: established this roadmap from shipped commits and the agreed
  philosophy. Configuration and provider compatibility come before the application
  SDKs; app migrations and the public alpha now have explicit completion criteria.
