#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/../.." && pwd)
consumer=$(mktemp -d /tmp/agent-bridge-consumer-XXXXXXXX)
# Retain the isolated installation and database for inspection after the check.
echo "Consumer directory: $consumer"
cd "$root/host"
bun pm pack --ignore-scripts --filename "$consumer/agent-bridge.tgz"
cp "$root/target/debug/agent-bridge-host" "$consumer/agent-bridge-host"
cp "$root/host/tool-agent.fixture.ts" "$consumer/provider.ts"
rustc --edition=2024 "$root/tests/support/acp_fixture.rs" -o "$consumer/config-provider"
cp "$root/verification/consumer/main.ts" "$consumer/main.ts"
cp "$root/verification/consumer/package.json" "$consumer/package.json"
cd "$consumer"
bun install --ignore-scripts
# The compiler and ambient Bun types are build tooling; package imports resolve here.
"$root/host/node_modules/.bin/tsc" --noEmit --strict --skipLibCheck --target ESNext --module Preserve --moduleResolution Bundler --types bun --typeRoots "$root/host/node_modules/@types" main.ts
timeout 60s bun main.ts ./agent-bridge-host ./provider.ts ./config-provider
