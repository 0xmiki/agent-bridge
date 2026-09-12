# Packaged Rust and Bun consumers

Run on Linux with Bun 1.3.13 and the Rust development shell:

```sh
cargo build --locked --features host --bin agent-bridge-host --example rust_integration
bun install --cwd host --frozen-lockfile
bash verification/consumer/verify.sh
```

The script creates a fresh directory under `/tmp` and retains it for inspection.
It packages the current working tree, extracts the `.crate`, and installs the host
with `cargo install --path` from that archive. It checks all packaged Rust targets
and compiles the documented Rust integration example as a separate application
depending on the extracted crate. That application runs against a deterministic ACP
provider and verifies exact persisted history after reopening.

The Bun consumer installs a local tarball and imports only declared package entry
points. It exercises configuration, selected text context, validated JSON, tools,
questions, cancellation, reopened state, discovery, and native handoff/resume. The
repository supplies compiler tooling and cached Cargo dependencies, not runtime
source imports. This checks local artifacts, not registry resolution or a fresh
machine's dependency downloads. The installed host uses Cargo's default release profile.

`legacy-v1.sql` freezes the schema-1 layout from commit `b852ace` and seeds an
updated legacy record with its creation snapshot. It is a synthetic compatibility
fixture, not a database from an earlier published release. The check first verifies
that host readers refuse to migrate it. A separate Rust consumer explicitly opens
it for upgrade, verifies typed evidence and change backfill, and closes it. The
installed host then reads it. Raw record rows, application data, and `user_version`
must remain unchanged. Both entry points reject a future schema; wire checks reject
malformed requests and future versions while accepting version 1.

Finally, the existing 56-test Linux host suite runs against the installed binary,
including provider-descendant cleanup, pipe failures, cancellation, storage faults,
and SIGKILL recovery. Test sources and deterministic providers come from this
repository; this is not independent developer review. CI runs the same script.

## Opt-in live checks

The retained directory contains `live.ts` and `live-interactions.ts`, copied from
the existing host demos. They import the installed package. From that directory:

```sh
AGENT_BRIDGE_HOST="$PWD/installed/bin/agent-bridge-host" \
  timeout 180s bun live.ts "$PWD/live.sqlite3" "$PWD" /absolute/provider args
```

Use `live-interactions.ts` for selected-context and validated-result checks.
Use a new database path per run. These commands use the provider's configured
account and model quota. Codex checks request provider session deletion on close
(the pinned adapter archives); set `AGENT_BRIDGE_CODEX_TEST=1` for wrappers whose
path does not identify Codex. Deterministic verification does not run these checks,
authenticate accounts, publish packages, or download provider binaries.
