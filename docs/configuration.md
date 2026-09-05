# Model selection and run configuration

The M1 implementation exposes session options, applies validated changes between
runs, and records configuration attribution. The API remains provisional. Provider
compatibility beyond OpenCode is tracked separately in M2.

## Discover and select

```rust
use agent_bridge::ConfigValue;

let configuration = session.configuration();
// configuration.options contains ordered, labeled choices and current values.

let confirmed = session.set_model("provider/model-id").await?;
// Or choose a provider-defined option explicitly:
let confirmed = session.set_option("option-id", ConfigValue::Boolean(true)).await?;
```

Use values from the offered catalog. The examples above are placeholders, not known
model or option IDs. `set_model` requires exactly one option with the model category;
use `set_option` when categories are missing or ambiguous. Categories are UI hints
and do not grant authority. Group labels, descriptions, and provider ordering survive
normalization. Select values and booleans are supported; ACP boolean support is
advertised during initialization.

The adapter uses `session/set_config_option` and requires its response to confirm the
requested value. Unknown options and invalid values fail before a request is sent.
A provider rejection is returned explicitly. There is no automatic restart or
provider switch if the operation is unsupported.

Model changes apply between runs. A run exclusively borrows its session, so callers
cannot use the session setter while that run handle remains alive. Finish and drop
the run handle before changing settings. The application session and native session
IDs stay the same across supported model changes.

## What is recorded

Each default `RunSpec` contains a `RunConfiguration`:

| Field | Meaning |
| --- | --- |
| `requested` | Explicit selections successfully acknowledged on this session handle |
| `confirmed` | Last reliable provider-reported values at dispatch; `None` means unknown |
| `continuation` on `RunSpec` | The claimed continuation from which the native session originated |

Only keys present in `confirmed` have reported values. An empty map does not confirm
an unknown model. A provider report is not independent proof of which remote model
served every generation or delegated task. The library does not infer a model from
the executable name, agent version, or an application default.

Changing a model may reset other settings. The setter returns the complete current
catalog so the app can reflect those changes. For example, a requested high reasoning
level may become a provider-reported low level after changing models. Both values
remain attributable instead of pretending the request and report are identical.

Reports arrive during setup, acknowledged configuration changes, and unsolicited
`config_option_update` notifications. Setup and setter responses are processed in
dispatch order before following notifications. Idle notifications update the current
session state too. `session.info()` remains the initial native response; use
`session.configuration()` for current settings.

The recorded-run API freezes configuration before registering the run. If a different
report is observed before dispatch, the run is rejected rather than silently changing
that saved snapshot. Provider configuration changes during a run become separate
`agent_bridge/configuration_report` extension records containing normalized values.
They do not rewrite the dispatch snapshot. Catalog descriptions and full model lists
are not copied into every historical run or report.

## Uncertainty and cancellation

A pending configuration request blocks new runs and handoff. A timeout or malformed
report leaves configuration uncertain; the previous report must not be treated as
current confirmation. A later valid acknowledgement can restore known state.
Otherwise reconnect or explicitly re-establish configuration when the provider can
accept another request.

Dropping a setter future does not undo a request already sent. The response handler
continues tracking it, and a late acknowledgement can change the session settings.
There are no automatic configuration retries. The session-setup timeout also applies
to setters.

## Persistence and continuation

SQLite schema version 3 adds `config_json` and `continuation_id` to run registrations.
Existing runs from versions 1 and 2 migrate to unknown configuration with no inferred
continuation. Record JSON format remains version 1. New run configuration is immutable
and survives database reopen.

Runs on a resumed session reference its originating claimed continuation. Several
runs may share that origin; they do not claim it repeatedly. Registration validates
the continuation's application session and slot. Historical links remain readable
after a later handoff supersedes that continuation.

Resume uses newly reported provider settings. `requested` starts empty on the new
session handle; earlier explicit choices remain in earlier run snapshots. The runtime
does not reapply old selections behind the caller's back.

## Verification

Tests cover grouped selectors, booleans, invalid values, dependent settings, late
acknowledgements, immediate post-setup updates, unknown reports, immutable run data,
continuation ownership, and migrations preserving legacy data.

A real OpenCode check switched from `opencode/big-pickle` to
`opencode/mimo-v2.5-free` within one native session. The second model recalled a phrase
from the first turn, and both run configurations survived SQLite reopen. This checks
the provider's model-selection interface and reported state, not independent model
identity attestation.

The example first supports listing models without making a prompt request:

```sh
cargo run --features acp,sqlite --example acp_models -- \
  /tmp/agent-bridge.sqlite3 /absolute/workspace list - opencode acp
```

Replace `list -` with two distinct offered model IDs to run the memory check. That
mode makes real model calls and saves the two runs. The example limits the workflow
to 120 seconds and dismisses permission requests.

Next questions include legacy provider configuration APIs, configuration changes
inside provider-managed subagents, and durable records of failed setup attempts.
See the [milestones](../milestone.md) before expanding those boundaries.

Reference: [ACP session configuration options](https://agentclientprotocol.com/protocol/v1/session-config-options).
