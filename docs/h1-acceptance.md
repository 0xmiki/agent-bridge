# H1 acceptance

Reviewed September 8, 2026. H1 is complete for the Linux hosted integration proof,
with direct Rust usage documented and fixture-tested. This is a milestone decision,
not a public alpha release, an independent reassessment, or a new architecture score.

## Acceptance evidence

| H1 requirement | Evidence |
| --- | --- |
| Applications do not implement ACP parsing/routing/process supervision | Rust host and typed Bun client; compiled direct Rust example using the library adapter |
| Concurrent ownership and continued sessions | Distinct interleaved fixture streams, retained session values, correlated outcomes, cancellation, permission rejection, failed-start recovery |
| Saved application state | Exact reopened history, coalesced change cursors, schema backfill/rollback tests, projection checkpoints, typed receipt readers |
| Bounded failure behavior | Shared DELETE/WAL tests, read-only readers, explicit lock failures, storage-worker deadlines/capacity, late-write uncertainty, stalled-reader and provider-tree cleanup |
| Independent observers | Scoped lag, byte/event limits, unsubscribe/admission, immutable events, missed-permission recovery without replay |
| Real provider checks | Recorded OpenCode/Codex host workflows and verified cleanup of disposable Codex history |
| Reviewable integration contract | [Integration guide](integration.md), [errors and ownership](errors-and-ownership.md), executable Rust example tested on success and provider error |

The final local checks passed 189 Rust tests, twenty-five Bun host tests with 684
assertions, Clippy, rustfmt, TypeScript checking, dependency-free core compilation,
and standalone receipt-reader compilation. CI runs these deterministic paths and
builds the executable Rust example before its integration tests. Live provider results
and their tested versions are recorded in [host verification](../host/verification.md);
the final documentation increment used fixtures and created no Codex threads.

## Boundaries carried forward

- The hosted proof is Rust host plus Bun client on Linux. Direct Rust is a lower-level
  embedding path and exposes typed ACP adapter events. It does not inherit host
  storage deadlines or subscriber queues. “No ACP plumbing” does not mean those
  advanced Rust types are hidden.
- Storage faults are controlled blocking-store tests. Kernel-level uninterruptible
  I/O and platform-specific termination remain release evidence, not established
  hard real-time guarantees.
- Observer lag is isolated in the Bun client. Shared transport failure can affect
  the entire host; provider-output fairness under sustained load is not established.
- Provider declarations are not capability proofs. Claude authenticated checks are
  deferred. No Windows/macOS lifecycle support is claimed from Linux tests.
- Saved history and late-write evidence do not establish durable execution. Enumerating
  unfinished work, crash-boundary tests, and explicit recovery choices remain H3.
- Host tools/questions/context/result workflows are H2. Packages, independent consumer
  adoption, and advertised-OS verification remain H4. There is no released Tauri plugin
  or Rust client for the host protocol.

These boundaries make the acceptance scope explicit rather than turning unfinished
release work into implied support. If a consumer exposes a failure of an accepted
contract, reopen the relevant H1 item and retain the regression test.

Next is H2: expose one application tool through the host and client, derive its actual
MCP binding from the approved grant, and prove invocation/cancellation boundaries
before expanding the interaction API.
