# agent-bridge

Shared integration for connecting applications to installed AI agents.

Early Rust library for a provider-independent execution model. The API is still
being developed.

Read the [philosophy](philosophy.md) and [working model](docs/model.md). Both will
evolve as we test the implementation against real agents.

Implemented so far: typed identifiers, slots and sessions, explicit context
references, typed record envelopes, and an in-memory run lifecycle. The optional
ACP adapter launches installed agents, initializes connections, reports advertised
capabilities, and manages shutdown. Prompt execution, persistence, grants, and
continuation handling are still to come.

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

The core has no runtime dependencies with default features. Enable `acp` to use
the official ACP Rust SDK and Tokio. See [ACP connection notes](docs/acp.md) for
the API, tested behavior, and current limits.
