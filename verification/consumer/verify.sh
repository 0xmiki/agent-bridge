#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/../.." && pwd)
consumer=$(mktemp -d /tmp/agent-bridge-consumer-XXXXXXXX)
# Retain the isolated installation and database for inspection after the check.
echo "Consumer directory: $consumer"
cd "$root"
# Packaging may include this uncommitted verification change; never publishes.
cargo package --locked --allow-dirty --no-verify
tar -xzf "$root/target/package/agent-bridge-0.1.0.crate" -C "$consumer"
cargo install --locked --offline --path "$consumer/agent-bridge-0.1.0" --features host --bin agent-bridge-host --root "$consumer/installed" --target-dir "$root/target"
cargo check --locked --offline --manifest-path "$consumer/agent-bridge-0.1.0/Cargo.toml" --all-features --all-targets --target-dir "$root/target"
cp "$root/verification/consumer/Cargo.toml" "$consumer/Cargo.toml"
cp "$root/verification/consumer/upgrade.rs" "$consumer/upgrade.rs"
cp "$root/examples/rust_integration.rs" "$consumer/rust_integration.rs"
cp "$root/verification/consumer/legacy-v1.sql" "$consumer/legacy-v1.sql"
cp "$root/verification/consumer/compatibility.ts" "$consumer/compatibility.ts"
cp "$root/host/example.ts" "$consumer/live.ts"
cp "$root/host/interactions-example.ts" "$consumer/live-interactions.ts"
cd "$root/host"
bun pm pack --ignore-scripts --filename "$consumer/agent-bridge.tgz"
cp "$root/host/tool-agent.fixture.ts" "$consumer/provider.ts"
rustc --edition=2024 "$root/tests/support/acp_fixture.rs" -o "$consumer/config-provider"
cp "$root/verification/consumer/main.ts" "$consumer/main.ts"
cp "$root/verification/consumer/package.json" "$consumer/package.json"
cd "$consumer"
cargo build --offline --target-dir "$root/target" --bins
timeout 60s "$root/target/debug/packaged-rust-consumer" "$consumer/rust.sqlite3" "$consumer" "$consumer/config-provider" chat
bun install --ignore-scripts
# The compiler and ambient Bun types are build tooling; package imports resolve here.
"$root/host/node_modules/.bin/tsc" --noEmit --strict --skipLibCheck --target ESNext --module Preserve --moduleResolution Bundler --types bun --typeRoots "$root/host/node_modules/@types" main.ts compatibility.ts live.ts live-interactions.ts
timeout 60s bun compatibility.ts ./installed/bin/agent-bridge-host "$root/target/debug/upgrade-consumer"
timeout 60s bun main.ts ./installed/bin/agent-bridge-host ./provider.ts ./config-provider
# Reuse the full Linux lifecycle/fault suite against the installed artifact.
cd "$root"
AGENT_BRIDGE_HOST="$consumer/installed/bin/agent-bridge-host" bun test host/client.test.ts
