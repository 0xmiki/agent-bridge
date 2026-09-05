# agent-bridge

Shared integration for connecting applications to installed AI agents.

Early Rust library for a provider-independent execution model. The API is still
being developed.

Read the [philosophy](philosophy.md) and [working model](docs/model.md). Both will
evolve as we test the implementation against real agents.

Implemented so far: typed identifiers, slots and sessions, explicit context
references, typed record envelopes, and an in-memory run lifecycle. The optional
ACP adapter launches installed agents, creates sessions, streams text runs and tool
activity, routes permission decisions, and handles cancellation. It can supply
existing MCP server configuration when creating a session. Recorded runs assemble
portable transcripts through a local store, with an in-memory implementation.
Disk persistence, grant policies, and cross-process continuation are still to come.

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
