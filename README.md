# agent-bridge

Shared integration for connecting applications to installed AI agents.

Early Rust library for a provider-independent execution model. The API is still
being developed.

Read the [philosophy](philosophy.md) and [working model](docs/model.md). Both will
evolve as we test the implementation against real agents.

Implemented so far: typed identifiers, slots and sessions, explicit context
references, typed record envelopes, and an in-memory run lifecycle. Provider
connections, persistence, grants, and continuation handling are still to come.

## Development

Enter the development shell:

```sh
nix-shell
```

Check the crate:

```sh
cargo check
cargo test
cargo fmt --check
cargo clippy -- -D warnings
```

The shell includes Rust, Cargo, rustfmt, Clippy, rust-analyzer, GCC, and
pkg-config. Rust sources are available through `RUST_SRC_PATH`.

Run the model example, which simulates two execution assignments in one session
without launching an agent:

```sh
cargo run --example model
```
