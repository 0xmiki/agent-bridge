# Host verification

September 7, 2026, Linux/NixOS, Bun 1.3.13. These results apply to the experimental
host in this change, not to every feature of the Rust library.

Local deterministic checks passed: 175 Rust tests (`--all-features --all-targets`),
Clippy with warnings denied, rustfmt, default-feature-free compilation, TypeScript
typechecking, and five Bun integration tests (18 assertions). Linux CI is configured;
these are local results, not a claim about a remote CI run.

| Provider | Versions | Host example outcome |
| --- | --- | --- |
| OpenCode | 1.18.25 | Two completed turns, recalled phrase, third run cancelled, 15 records reopened |
| Codex | CLI 0.153.4, codex-acp 1.10.0 | Two completed turns, recalled phrase, third run cancelled, 19 records reopened |
| Claude | — | Authenticated checks deferred; not verified through this host |

Both checks used `host/example.ts`, disposable SQLite files, and a disposable workspace.
History reopened through a fresh host without creating another provider session.
Record counts differ with provider output; they are observations, not API expectations.

An initial Codex attempt recalled the phrase twice and failed the demo's exact-output
assertion. The demo now checks phrase inclusion, which is the intended context test.
The repeated text reinforces the need for typed message boundaries in H1. The repeated
check completed all steps. Cancellation may lose to normal completion; both observed
checks here ended cancelled.

Storage-lock isolation, slow-reader shutdown, forced-crash recovery, host application
tools, cross-platform lifecycle, and package installation remain unverified. See the
[quality gates](../docs/quality-gates.md).
