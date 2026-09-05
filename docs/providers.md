# Installed providers

The optional `providers` feature adds provider definitions, installation discovery,
profile scopes, and a connection driver. It includes `acp` and its record support.
This is the first M2 increment; the API remains provisional.

## The boundary

A **definition** describes a provider's launch convention and setup guidance. A
**profile** selects paths, environment overrides, and a continuation namespace.
**Discovery** resolves a launch without executing commands. A **driver** explicitly
connects that resolved launch and reports protocol evidence.

OpenCode, Codex, and Claude currently use the same `AcpDriver`. Custom definitions
can use it too. `ProviderDriver` has an associated connection type, so a future
driver can preserve capabilities that ACP cannot express. This is a launch and
connection contract: session/run APIs still expose ACP types. The neutral host and
TypeScript contracts belong to M6.

```rust,no_run
use agent_bridge::providers::{AcpDriver, ExecutableSearch, ProviderDefinition, ProviderDriver};
use agent_bridge::{SessionId, SlotId};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let inspection = ProviderDefinition::opencode()
    .profile("personal")
    .inspect(&ExecutableSearch::current());
if !inspection.is_launchable() {
    // Present issues and suggested setup actions in the application's UI.
    println!("{:?}", inspection.report);
    return Ok(());
}
let resolved = inspection.into_resolved().map_err(|_| "setup required")?;
let provider = AcpDriver.connect(resolved).await?;
let session = provider.new_session(
    SessionId::new("conversation-1")?,
    SlotId::new("assistant")?,
    "/absolute/workspace",
    vec![],
).await?;
// Use session.start_recorded_run(), configuration(), set_model(), etc.
drop(session);
provider.shutdown().await?;
# Ok(())
# }
```

Changing the definition to `codex()` or `claude()` preserves this call sequence.
Setup and model availability still depend on the installed provider.

## Setup evidence

`InstallationReport` distinguishes missing CLIs, missing adapters, missing Node,
invalid paths, unreadable/non-executable files, and unsupported launchers.
`is_launchable()` means there is a resolved command to try. Discovery neither
connects nor checks authentication.

Connection checks ACP protocol negotiation and, for Claude JavaScript launches,
the declared Node 22 minimum. The version probe is bounded to three seconds and
128 output bytes. There is no guessed adapter-version allowlist. The report keeps
the provider's reported name/version and advertised capabilities for inspection.
An advertised feature has not necessarily passed a real workflow check.

Authentication starts `Unknown`. A structured ACP authentication-required error
sets it to `Required`; error text is never parsed to infer login status. Session
creation can succeed before authentication is checked. For errors returned by a
session or run, call `provider.classify_error(error)` to update that evidence.
Even prompt success does not attest to an account identity or future model access.

Suggested `SetupAction`s contain descriptions, commands, and documentation links.
The library does not execute them or initiate login. Supported setup sources:

- [OpenCode ACP](https://opencode.ai/docs/acp/): `opencode acp`, with model-provider
  setup through OpenCode.
- [Codex ACP adapter](https://github.com/agentclientprotocol/codex-acp):
  `codex-acp`. Its npm package includes a Codex runtime; a separate CLI is
  informational. Existing CLI authentication uses [Codex login](https://developers.openai.com/codex/auth).
- [Claude ACP adapter](https://github.com/agentclientprotocol/claude-agent-acp):
  `claude-agent-acp`, backed by the Claude Agent SDK. JavaScript distribution
  requires Node 22+. Use [Claude's supported authentication](https://code.claude.com/docs/en/authentication).

## Paths and profiles

`ExecutableSearch::current()` searches absolute PATH entries and common per-user
locations. `from_directories()` supplies a deterministic search list. Discovery
does not run a login shell or package manager. Relative PATH entries are excluded.

Use `.executable("/absolute/path")` for an explicit executable, or
`.node_script("/absolute/adapter/dist/index.js")` for a JavaScript entry point.
Explicit paths never fall back to another installation. Recognized Node shebangs
are launched with a resolved absolute Node executable. Node is not required for
native standalone adapters. Missing Node requires local Node installation and a
search path containing it.

`.arg(value)` appends a literal argument; `.env(key, value)` overrides the child
environment. These are not shell expressions. Environment overrides do not change
discovery's search list. Profiles and resolved launches deliberately lack Debug
and serialization implementations because overrides can contain credentials.

Profile scopes are application-supplied account/configuration namespaces, combined
with the provider ID using an unambiguous encoding. Use different scopes when
account or provider state directories differ. A scope is not an account identity
check. Native handoffs retain the existing strict provider/scope compatibility
rules in [continuations](continuations.md). Access the current driver's continuation
API through `provider.acp()`.

Only Linux has been exercised. Windows `.cmd`/`.bat` launchers return an explicit
unsupported-launcher issue; use a native executable or `node_script` instead.
Other shebang conventions and package-manager-specific wrappers may need explicit
paths. Cross-platform packaging remains an M8 release requirement.

## Verification

Run read-only discovery with:

```sh
cargo run --features providers --example providers
```

Passing a provider ID explicitly launches it. Optional workspace and prompt
variables extend the probe to session creation and a real text run. See the
[pinned adapter recipe and evidence](../verification/adapters/README.md).

Fixture tests cover shared launch/session behavior, structured authentication and
protocol errors, discovery, scope isolation, and runtime checks. They do not prove
vendor tools, cancellation, model switching, or continuation interoperability.
The opt-in `provider_compat` example now checks those workflows against real
providers. OpenCode and Codex have passed tool, cancellation, model-continuity,
and native-resume checks. The evidence table records permission checks separately;
Claude's generation checks remain deferred because authenticated access is unavailable.
The user accepted progressing to M3 with OpenCode and Codex verified. Claude's
workflow support must not be advertised as equivalently verified until its suite runs.
