# agent-bridge

Shared integration for connecting applications to installed AI agents.

Early Rust library for a provider-independent execution model. The API is still
being developed.

Read the [milestones](milestone.md) for current status and the next implementation
target. The [philosophy](philosophy.md), [working model](docs/model.md), and roadmap
will evolve as we test the implementation against real agents.

Implemented so far: typed identifiers, slots and sessions, explicit context
references, typed record envelopes, and an in-memory run lifecycle. The optional
ACP adapter launches installed agents, creates sessions, streams text runs and tool
activity, routes permission decisions, and handles cancellation. It can supply
existing MCP server configuration when creating a session. Recorded runs assemble
portable transcripts through memory or SQLite stores. Native ACP sessions can be
handed off and resumed through single-use continuations. Grant policies, portable
context restoration, and uncertain-outcome reconciliation are still to come.

Model and option changes can be applied between runs. Requested selections and
provider-reported settings are stored with each run; later configuration reports
do not rewrite that history. See [configuration](docs/configuration.md).

[Context preparation](docs/context.md) resolves selected immutable records and
resource revisions while preserving instruction roles. Recorded ACP runs can
explicitly append supported text context and persist input receipts before dispatch.
SQLite can retain immutable resource revisions with shared blobs. Explicit image
delivery adds native image blocks and receipt digests without copying base64 into
each saved transcript. Support still depends on the selected provider model.
[Restoration policies](docs/restoration.md) explicitly choose native resume or a
new provider session with selected portable context, and retain a report of that choice.

The `providers` feature adds OpenCode, Codex, and Claude launch definitions,
read-only discovery, explicit profiles, and setup diagnostics through a shared
ACP driver. See [installed providers](docs/providers.md) for the API and current
compatibility evidence. Inspect local installations with
`cargo run --features providers --example providers`.

## Development

Enter the development shell:

```sh
nix-shell
```

Check the crate:

```sh
cargo check
cargo test --all-features --all-targets
cargo fmt --check
cargo clippy --all-features --all-targets -- -D warnings
```

The shell includes Rust, Cargo, rustfmt, Clippy, rust-analyzer, GCC, and
pkg-config. Rust sources are available through `RUST_SRC_PATH`.

Run the model example, which simulates two execution assignments in one session
without launching an agent:

```sh
cargo run --example model
```

Initialize an installed ACP agent without creating a session or sending a prompt:

```sh
cargo run --features acp --example acp_connect -- opencode acp
```

Send a real prompt using the agent's configured model. Use an existing absolute
workspace path:

```sh
cargo run --features acp --example acp_chat -- /path/to/workspace \
  "Say hello without using tools." opencode acp
```

This example dismisses permission requests and has a 60-second timeout. Applications
can present the offered options and respond explicitly through the run API.
It also records the run in memory and reads its history after connection shutdown.

The core has no runtime dependencies with default features. Enable `acp` to use
the official ACP Rust SDK and Tokio. See [ACP connection notes](docs/acp.md) for
the API, tested behavior, and current limits.

Use the `records` feature independently for portable payloads and the local storage
contract. See [records and storage](docs/records.md) for checkpoints, identity rules,
and the distinction between local recording and durable execution.

Enable `sqlite` for disk-backed records. It works independently of ACP:

```sh
cargo run --features sqlite --example sqlite_history -- /tmp/agent-bridge.sqlite3
```

This example writes a record, closes the store, and reads it after reopening.
For a real agent, use `SqliteStore::open(path)` in place of `MemoryStore` when
calling `start_recorded_run`. See [SQLite storage](docs/sqlite.md) for migrations,
the versioned format, and restart behavior.

Native session handoff is separate from saved history. See
[provider continuations](docs/continuations.md) for the single-use claim model and
the real two-process example.
