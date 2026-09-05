# agent-bridge

Shared integration for connecting applications to installed AI agents.

The project currently contains an initial Rust library crate.

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
