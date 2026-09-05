# Application tools

The optional `tools` feature adds a typed asynchronous tool registry. `mcp` adds
an MCP adapter using the official Rust SDK, `rmcp`. Neither feature is required
by the core or record store. This is the first M4 increment.

## Register a declaration and handler

```rust,ignore
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Lookup { key: String }

registry.register::<Lookup, _, _, _>(
    ToolRef { name: "project_lookup".into(), revision: "v1".into() },
    "Look up a project visible in this session",
    |invocation, input| async move {
        // Use the host-bound invocation scope for application authorization.
        lookup_project(invocation.scope.session, input.key).await
    },
)?;
```

The input schema is generated from the same Rust input type through Schemars.
Inputs must have an object schema; scalar roots are rejected. Serde checks arguments
before invoking the handler, while domain validation remains in application code.
Use `deny_unknown_fields` where extra properties should fail. Custom serde/schema
implementations remain the application's responsibility.

Handlers return `Result<O, ToolError>` with `O: Serialize`. The registry serializes
successful values as JSON. Registration alone authorizes nothing. Names are unique
within a registry; a duplicate name cannot silently replace an existing revision.
Use a new registry/binding when upgrading a tool's implementation, schema, or meaning.

## Bind execution authority

`ToolGrant` identifies the issuer, subject, application session/slot scope, and
permitted tool names/revisions. A `ToolInvocation` contains host-owned identity and
scope plus a cancellation token. Before calling a handler, the registry checks:

- Grant issuer matches the invocation's host.
- Grant subject matches the invocation's actor.
- Session and slot match the granted scope.
- The named tool is registered and its exact revision is granted.

`catalog(grant, invocation)` exposes only granted, registered revisions. Handlers
receive the trusted invocation context separately from the model's arguments.
Neither instruction text nor tool discovery creates a grant.

This is an application dispatch boundary, not an external identity provider.
Authenticate callers before constructing host invocations and grants. Handlers still
enforce domain data access. A grant is a reusable session/slot allowlist, not a
single-use approval or a durable job claim.

Provider permission prompts remain separate. Answering an ACP permission request
does not issue or broaden a `ToolGrant`. Applications that require their own per-call
approval can await that decision inside an asynchronous handler; this increment
does not supply a UI policy or correlate every provider permission with a host call.

## MCP adapter

`McpToolServer::new(registry, grant, scope, actor, host)` validates and binds one
registry to a trusted scope. MCP callers cannot replace it through request arguments
or metadata. Its catalog contains only the granted revisions. Each call is checked
again before dispatch.

`serve_stdio()` runs the SDK transport until disconnect, reserving stdin/stdout for
MCP. The server also implements `rmcp::ServerHandler` for hosts using another SDK
transport. The built-in helper covers stdio, not an authenticated HTTP deployment
or a Tauri frontend connection.

Tool results are JSON rendered in an MCP text content block. Handler, argument,
grant, and cancellation failures become tool-level error results. Unsupported
pagination cursors and MCP task requests are protocol errors. The adapter does not
advertise asynchronous MCP task execution or dynamic catalog changes.

The SDK's request cancellation token reaches the registry and handler. A cancelled
invocation does not start new handler work; cancellation during an asynchronous
handler drops its pending future. This does not undo completed side effects, stop
detached tasks, or preempt blocking code. Keep blocking work off runtime/UI threads
and pass cancellation to application operations where needed.

## Real provider example

```sh
cargo run --features acp,mcp --example application_tools -- \
  /absolute/disposable-workspace opencode acp
```

The example launches itself in a dedicated MCP-server mode. That mode registers a
typed project lookup, grants only that tool, and binds the parent application's
session and slot. The handler generates a verification value and writes an execution
log. The parent checks the agent's answer against that log and verifies recorded ACP
tool activity. The value is not supplied in the prompt or process arguments.

This subprocess example owns its own small application state. A Tauri application
will need an appropriate host transport or IPC path to reach live UI-process state;
that integration belongs to M6. Do not treat launching a subprocess as sharing Rust
closures or memory with the parent.

The example uses a 90-second timeout and dismisses native permission requests.
It passed with the existing automatic-approval configurations on September 6, 2026
through OpenCode 1.18.25 and Codex ACP 1.10.0 using local Codex 0.153.4. Claude remains
deferred. The registry grant is enforced independently of those provider defaults.

Tests cover typed argument rejection, scope/revision checks, filtered discovery,
duplicate registration, cancellation, MCP request routing, and spoofed metadata.
The registry itself does not persist invocation receipts, deduplicate executions,
or reconcile uncertain effects. Existing ACP recording preserves provider-observed
tool activity; authoritative host execution bookkeeping belongs to M5.

Next M4 work covers structured questions, application approval orchestration,
execution relationships, child authority, and concurrency/delegation limits.
