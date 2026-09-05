# Provider verification adapters

These pinned npm dependencies are development tools, not dependencies installed
by agent-bridge. From this directory, use Node 22+ and run:

```sh
npm ci --ignore-scripts
```

From the repository root, build the common probe:

```sh
nix-shell --run 'cargo build --features providers --example providers'
```

With no arguments it only inspects installations. To launch a local adapter:

```sh
target/debug/examples/providers codex \
  "$PWD/verification/adapters/node_modules/@agentclientprotocol/codex-acp/dist/index.js"
target/debug/examples/providers claude \
  "$PWD/verification/adapters/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"
target/debug/examples/providers opencode
```

Set `AGENT_BRIDGE_PROBE_WORKSPACE` to an existing absolute directory to create a
session. Also set `AGENT_BRIDGE_PROBE_PROMPT` to send a real prompt using the
provider's configured model. The text probe dismisses permission requests and
times out after 60 seconds. Failures exit nonzero after connection shutdown.
Running it can use the configured account's model quota.

For Codex, `AGENT_BRIDGE_CODEX_PATH` optionally selects an explicit local Codex
binary through the adapter's `CODEX_PATH` override. Otherwise the adapter chooses
its runtime. No login or credential installation is performed by the probe.

## Observed September 5, 2026

Linux/NixOS, Node 24.15.0. Prompt: `Reply with exactly: provider connected. Do not
use tools.` All three use the same `providers` example and `AcpDriver`.

| Check | OpenCode 1.18.25 | Codex ACP 1.10.0 | Claude ACP 0.75.1 |
| --- | --- | --- | --- |
| ACP initialization | Passed | Passed | Passed |
| Session creation and configuration report | Passed | Passed | Passed |
| Text prompt | Passed | Passed | Authentication required |
| Reported model selection | opencode/big-pickle | gpt-6-astra | default |
| Real two-model context continuity | Passed in M1 | Not tested | Not tested |
| Native resume | Passed in M0 | Not tested | Not tested |
| Real tool/permission/cancellation suite | Not tested | Not tested | Not tested |

Codex used local codex-cli 0.153.4 via the explicit override. Claude used the
adapter's Claude Agent SDK 0.3.257. Claude returned ACP's structured authentication
error on the first prompt, despite accepting session creation and listing no auth
methods. The bridge correctly reported `AuthenticationState::Required`.

All three advertised images, embedded context, session load/resume, and HTTP MCP.
OpenCode and Claude also advertised SSE MCP; Codex did not. These declarations are
not workflow verification. Model names are provider-reported configuration, not
independent model identity attestation.

The earlier OpenCode model/resume checks use the ACP examples documented in
[configuration](../../docs/configuration.md) and
[continuations](../../docs/continuations.md). The full common real-provider suite
is still pending. Claude needs supported local authentication before its
generation-dependent checks can run. Extend this evidence as M2 proceeds.
