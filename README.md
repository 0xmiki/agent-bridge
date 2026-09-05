# agent-bridge

Shared integration for connecting applications to installed AI agents.

Early Rust library for a provider-independent execution model. The API is still
being developed.

Read the [philosophy](philosophy.md) and [working model](docs/model.md). Both will
evolve as we test the implementation against real agents.

## Development

Enter the development shell:

```sh
nix-shell
```

Check the crate:

```sh
cargo check
cargo fmt --check
cargo clippy -- -D warnings
```

The shell includes Rust, Cargo, rustfmt, Clippy, rust-analyzer, GCC, and
pkg-config. Rust sources are available through `RUST_SRC_PATH`.
