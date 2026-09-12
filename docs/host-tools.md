# Hosted application tools

This is H2's first tool workflow. Handlers execute in the Bun application; the Rust
host owns schema validation, grants, MCP configuration, invocation routing, and receipts.
The complete runnable example is [tools-example.ts](../host/tools-example.ts).

```ts
const lookup = defineTool({
  name: "project_lookup",
  revision: "v1",
  description: "Look up a project visible in this session",
  input: projectLookupInput,
  execute: ({ projectId }, { scope, signal }) =>
    projects.getVisibleProject(scope.sessionId, projectId, { signal }),
});

const host = new BridgeHost(binary, { tools: [lookup] });
const session = await host.createSession({
  ...providerConfiguration,
  allowTools: [lookup],
});
const run = session.run("Look up this project.");
```

Import `BridgeHost` and `defineTool` from `host/client.ts`. A `ToolInput<I>` adapter
provides `jsonSchema` and `parse(unknown): I`, so a schema library can supply the
application type. No framework-specific schema adapters ship yet. The included example
uses a small explicit adapter. Input parsing may add domain validation, but must not
coerce, strip, or add fields compared with the JSON validated by Rust. The helper
captures the declaration/parser/handler at definition time. Application data accessed
by the handler can remain live.

Registration grants no execution access. `allowTools` selects exact registered names
and revisions for the new session. Empty grants expose no application MCP server.
Unknown, duplicate, or stale selections fail before provider launch. Declarations
are immutable after configuration or session creation; changing implementations or
schemas requires a new host/catalog, while different allowlists can use new sessions.

## Binding and routing

The host assigns session and slot IDs, creates a `ToolGrant`, and derives the actual
ACP MCP configuration from that grant. The provider launches the host binary in
MCP-helper mode. It connects over a Unix socket in a private directory, using a fresh
capability tied to that binding. The existing Rust MCP server and registry enforce
the scope and exact tool revision. Call arguments and MCP metadata cannot replace them.

The parent forwards admitted calls to the application's handler using the existing
host wire. These control messages are separate from run observers; closing a view
does not discard a tool invocation. Every callback has a host-issued invocation ID,
binding ID, and session/slot scope. Responses must match all of them. Invalid responses
leave the legitimate pending call usable; duplicate and late responses are rejected.

The local binding assumes trusted application/CLI processes. Handlers still enforce
domain data access using their trusted context, and provider built-in tools retain
their own permission policy. A bridge grant is a session/slot allowlist, not an
external identity provider, OS sandbox, or single-use approval.

## Evidence and cancellation

Each admitted application call writes version-1 `agent_bridge/tool_invocation`
receipts, separately from provider-observed ACP tool activity:

| State | Meaning |
| --- | --- |
| `dispatch_attempted` | Input and invocation identity persisted before forwarding to the application |
| `returned` | The application returned a success value or error, and that outcome was recorded before the MCP response |
| `unknown` | Cancellation, timeout, disconnect, or recording failure prevented a recorded return outcome |

Receipts carry trusted scope, issuer, subject, tool revision, binding, and invocation
identity. Their record `run_id` is null: MCP does not attest to a parent run or native
subagent, so the bridge does not guess from arrival time or tool names. Typed receipt
readers expose the evidence. Input and outcome JSON are retained once in the original
payload, preserving their ordinary JSON number types.

Handlers receive an `AbortSignal`. Cancellation does not undo side effects or preempt
blocking application code. A handler that ignores abort retains its client capacity
slot until it actually returns. Late results cannot satisfy another invocation.

An uncertain invocation retires its MCP binding and stops other outstanding activity
on that binding. Further runs using it fail with `tool_binding_retired`; a fresh
session/binding is required after the application decides how to proceed. The bridge
does not replay the uncertain call. This prevents a provider retry from reusing the
same endpoint after an unknown outcome. A fresh session is not proof that repeating
an external side effect is safe. The application owns idempotency and reconciliation;
[restart discovery](host-recovery.md) exposes saved uncertainty without replaying it.

An ordinary returned handler error is still a returned result, not a rollback claim.
A normal agent completion likewise does not establish tool success. Inspect invocation
receipts. If recording dispatch fails, the handler is not called. If recording the
return fails, the provider receives an error and the binding becomes uncertain/retired.

## Initial limits and protocol

- Sixteen registered tools, sixteen admitted calls globally, four per binding, and
  four MCP connections per binding.
- Input/result envelopes are bounded to 64 KiB; application success values to 60,000
  bytes. JSON depth is limited to sixteen and unsafe JavaScript integers are rejected.
- `toolTimeoutMs` defaults to 30,000, or 120,000 with hosted questions enabled, with
  a supported range of 50–300,000. It bounds
  waiting for the application response after dispatch, not every earlier storage step.
- Store queues now admit eight pending jobs for the recorder and tool callbacks.
  The existing twelve-worker budget and 500 ms storage wait policy remain.

Runtime schemas compile once with the optional `dynamic-tools` feature and the
`jsonschema` crate with external resolvers disabled. The initial Draft 2020-12 subset
supports object/array/primitive types, properties, required fields, additionalProperties,
items, enums/constants, numeric/string/collection bounds, uniqueItems, title and
description. Root schemas must be objects. References, regex/format assertions,
combinators, coercion, and defaults are rejected. Schemas are capped at 64 KiB,
sixteen schema levels, and 256 schema nodes. Generic Rust `register<I>` remains separate.

Clients opt into callback protocol 1 with `configure_tools` before session creation.
Only configured bindings produce `tool_call` and `tool_cancel` events. `tool_result`
acknowledges queuing, not persisted completion. This is an opt-in extension of host
wire version 1; old clients with no tools receive no new callback events.

## Verification and remaining work

```sh
bun host/tools-example.ts /tmp/bridge-tools.sqlite3 /absolute/workspace /absolute/opencode acp
```

The example verifies a fresh application-generated token and reopened receipts.
Recognized Codex launches enable test-thread cleanup; custom wrappers must set
`AGENT_BRIDGE_CODEX_TEST=1`. The pinned adapter archives those threads from active history.

Fixtures exercise the real stdio helper and MCP server, filtered discovery, startup
call rejection, schema failures, spoofed metadata, cross-binding capabilities, scoped
and late results, deadlines, recording failures, and provider-initiated retry rejection.
Live OpenCode/Codex evidence is recorded in [host verification](../host/verification.md).

[Hosted questions](host-questions.md) are now available through `context.ask`.
Context delivery, validated agent results, dynamic binding replacement, and post-crash
reconciliation remain later work. This helper currently requires Unix;
the verified platform is Linux.
