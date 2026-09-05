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
| Real two-model context continuity | Passed | Passed | Blocked by authentication |
| Native resume | Passed | Passed | Blocked by authentication |
| MCP tool result and streamed tool events | Passed | Passed | Blocked by authentication |
| Cancellation during a running MCP tool | Passed | Passed | Blocked by authentication |
| Client permission approval and dismissal | Passed | Passed | Blocked by authentication |

Codex used local codex-cli 0.153.4 via the explicit override. Claude used the
adapter's Claude Agent SDK 0.3.257. Claude returned ACP's structured authentication
error on the first prompt, despite accepting session creation and listing no auth
methods. The bridge correctly reported `AuthenticationState::Required`.

All three advertised images, embedded context, session load/resume, and HTTP MCP.
OpenCode and Claude also advertised SSE MCP; Codex did not. These declarations are
not workflow verification. Model names are provider-reported configuration, not
independent model identity attestation.

The shared workflow runner now verifies OpenCode and Codex model continuity and
native resume, in addition to the earlier OpenCode checks in
[configuration](../../docs/configuration.md) and
[continuations](../../docs/continuations.md). Claude also returned authentication
required when the shared MCP workflow was attempted. Its remaining workflows have
not run; they require supported local authentication.

## Shared workflow runner

Build with `cargo build --all-features --example provider_compat`. Its arguments are:

```text
provider_compat <opencode|codex|claude> <check> <absolute-workspace> [absolute-adapter.js]
```

Use a disposable workspace. Checks send real prompts, use the provider's configured
account, and keep evidence in a newly created temporary directory printed at startup.
The runner never installs credentials or changes global provider configuration.

| Check | Required evidence |
| --- | --- |
| `tools` | MCP fixture executed, tool events arrived, and the answer exactly matches the generated token. |
| `permissions` | An offered one-time approval was submitted for the fixture and its returned token matches. |
| `deny` | A fixture permission request was dismissed, and no fixture tool started. |
| `cancel` | The wait tool started, the bridge requested cancellation, and both native stop reason and bridge run status confirmed cancellation. |
| `models` | Two distinct selected models retained a unique phrase in one native session. |
| `resume` | A new provider process recalled the phrase after a single-use native handoff and SQLite reopen. |

The MCP fixture supports a token tool and a 30-second wait tool. It keeps a local
call ledger; the prompt does not contain the expected token. MCP checks require
`AGENT_BRIDGE_NODE` to name an absolute Node executable. Model checks require
`AGENT_BRIDGE_MODEL_A` and `AGENT_BRIDGE_MODEL_B`. The tested pairs were
`opencode/big-pickle` to `opencode/mimo-v2.5-free`, and `gpt-6-astra` to `gpt-5.6-luna`.

For example, from the repository root:

```sh
AGENT_BRIDGE_NODE="$(command -v node)" \
  target/debug/examples/provider_compat opencode tools /absolute/disposable-workspace

AGENT_BRIDGE_MODEL_A=gpt-6-astra AGENT_BRIDGE_MODEL_B=gpt-5.6-luna \
  target/debug/examples/provider_compat codex models /absolute/disposable-workspace \
  "$PWD/verification/adapters/node_modules/@agentclientprotocol/codex-acp/dist/index.js"
```

### Permission setup and interpretation

The default settings used here approved MCP tools automatically. Zero permission
requests do not pass the `permissions` or `deny` checks.

For OpenCode, run either check with the process-local
`OPENCODE_CONFIG_CONTENT='{"permission":{"*":"ask"}}'` override. This uses its
documented [permission rules](https://opencode.ai/docs/permissions/) and
[inline configuration](https://opencode.ai/docs/config/).

For Codex, the runner's permission checks set up this fixture through the adapter's
`CODEX_CONFIG` override, with `default_tools_approval_mode: "prompt"`, and select the
advertised `read-only` mode so the client receives requests instead of automatic
review. Existing native configuration is preserved except for the fixture's own
server entry. See [Codex MCP configuration](https://developers.openai.com/codex/config-reference).
ACP's MCP attachment fields have no approval-policy setting, so this native setup
is confined to verification. Ordinary tool and cancellation checks use ACP attachment.
Application authority configuration remains M4 work.

Permission subjects can be partial tool updates. The runner correlates their IDs
with earlier tool titles, which Codex needs. It approves only offered `AllowOnce`
options for the named fixture tools and dismisses other requests. This title-based
fixture policy is not an application authorization boundary.

### Limits

Every prompt has a 90-second timeout and failures exit nonzero. Successful workflows
write `result.json`; completed MCP observations also write it when their evidence
falls short. Errors before observation completes can leave only partial evidence.
Native resume keeps its SQLite file for inspection. Inspect the process exit status
as well as any result file, since shutdown can fail after workflow success.

Cancellation establishes the provider's acknowledgement while a tool was running.
It does not prove that every external tool side effect can be stopped or undone.
The ledger distinguishes a cancelled pending timer from a cancellation notification
received after a tool already finished. Native session load, images, HTTP/SSE MCP,
and provider-managed subagents are not covered by these checks.

Test the fixture without any provider or network access:

```sh
node --test verification/adapters/probe-tools.test.mjs
```

The test verifies token/ledger agreement, pending-tool cancellation, clean exit,
and unsupported-tool errors. It requires ordinary local subprocess execution.
