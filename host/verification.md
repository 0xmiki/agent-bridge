# Host verification

September 7, 2026, Linux/NixOS, Bun 1.3.13. These results apply to the experimental
host in this change, not to every feature of the Rust library.

At the initial host commit `511c090`, local deterministic checks passed: 175 Rust tests (`--all-features --all-targets`),
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

At that initial commit, storage-lock isolation, slow-reader shutdown, forced-crash
recovery, host application tools, cross-platform lifecycle, and package installation
were unverified. The follow-up evidence below updates that position. See the
[quality gates](../docs/quality-gates.md).

## Review-driven fault tests

The follow-up adds eleven host tests with 107 assertions. The 175 Rust tests, Clippy,
default-feature-free compilation, rustfmt, and TypeScript checks also pass locally.
The tests now force two distinct streams to interleave, verify each session recalls
its own value, and compare exact saved records and paginated reads after restart.
Permission checks reject foreign tokens, stale run IDs, invalid options, and duplicate
responses while proving the legitimate requests still complete.

An external SQLite transaction reproduced the old five-second lock wait. The host
now uses a 100 ms busy-handler budget. The test requires an explicit recording error
and unknown outcome within two seconds while another database's session can run and
cancel. It verifies no completed record is fabricated and no prompt is replayed when
the lock is released. Filesystem and mutex stalls remain outside this evidence.

A Rust consumer stops reading the host pipe without background JS stream buffering.
Linux tests check stdin EOF, stdout disconnect, a full pipe followed by EOF, and a
finite output burst with stdin kept open. The last two hung with the old writer.
All now exit with provider descendants stopped within five seconds. The finite-burst
case exercises the one-second output deadline without filling the output queue.
Output failure ends the whole host and may lose queued events.

The stricter live demo requires successful completed turns, no recording errors, one
terminal event, recall of a fresh UUID-bearing phrase, an actual cancellation attempt,
and exact transcript equality after restart. Only explicit terminal-race errors may
be tolerated by cancellation. Visible agent text is compared separately from reasoning;
only the cancelled run may retain interrupted records. Earlier attempts exposed those
two incorrect assumptions in the demo's assertions, which were corrected to match the
record model rather than dropping the checks.

Both stricter live checks passed on September 7, 2026 with the same provider versions
listed above. OpenCode completed two turns, cancelled the third, and exactly reopened
15 records. Codex did the same with 20 records. These are observed outcomes, not fixed
record-count expectations or a guarantee that cancellation always wins the race.

Forced host death, change subscriptions, shared-database contention policy, arbitrary
disk stalls, host application tools, and non-Linux lifecycle remain open.

## State synchronization and test cleanup, September 8

178 Rust tests, fourteen host tests with 128 assertions, Clippy, rustfmt,
default-feature-free compilation, and TypeScript checking passed locally.
Schema 7 tests cover coalesced old-record updates, overlapping snapshot pages,
database/session cursor validation, future positions, migration backfill, and rollback.
The client test saves a projection/cursor pair, verifies a failed refresh leaves it
unchanged, and resumes through a fresh host without duplicate transcript items.

OpenCode 1.18.25 and Codex CLI 0.153.4 through codex-acp 1.10.0 passed the stricter
live demo with typed projections and saved checkpoints. Both completed two turns
and cancelled the third; exact reopened state contained 13 and 20 records respectively.

The Codex test enabled `delete_session_on_close`. The ACP deletion request succeeded,
and a read-only check of the exact test thread in Codex's database confirmed
`archived = 1`. The pinned adapter maps deletion to archive, not permanent erasure.
Four older exact host-smoke matches were also archived through Codex's supported
app-server API after validating their workspace and first test prompt.

The fixture suite covers successful provider cleanup with bridge records retained,
and cleanup rejection causing a nonzero host exit. These checks do not establish
cleanup after abrupt host death or failure before the adapter returns a session ID.
