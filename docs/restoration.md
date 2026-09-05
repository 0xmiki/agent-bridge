# Context restoration

M3 now exposes `AcpConnection::restore(policy, store, resources, mcp_servers)`.
The caller must choose `RestorationPolicy::Native` or `RestorationPolicy::Portable`.
There is no automatic fallback. The result is a `RestoredSession`, and setup alone
does not send a prompt or establish successful context delivery.

## Native resume

```rust,ignore
let mut restored = connection.restore(
    RestorationPolicy::Native { continuation: saved_id },
    &store,
    &resources,
    mcp_servers,
).await?;
```

This delegates to the existing [single-use continuation contract](continuations.md).
The provider must advertise native resume, and the saved adapter, profile scope,
provider name, and provider version must match. Preflight errors leave the handle
unclaimed; errors after claiming leave it claimed. No history is read from the
resource store, replayed as text, or used to create a replacement native session.

The saved application session and slot remain attached to the resumed session.
Subsequent run specs retain the originating continuation ID. The setup report says
`native_context: reused_uninspected`: the provider resumed its own state, but the
bridge cannot attest its contents or completeness. This is not a portable snapshot.

## Portable selection

```rust,ignore
let mut restored = connection.restore(
    RestorationPolicy::portable(PortableRestore {
        policy: ContextPolicy::for_host(host_actor.clone(), selected_context.instructions.clone()),
        session_id: conversation_id,
        slot_id: destination_slot,
        cwd: workspace,
        manifest: selected_context,
        limits: ContextLimits { max_items: 100, max_resource_bytes: 4 * 1024 * 1024 },
        max_prompt_bytes: 6 * 1024 * 1024,
        mode: ContextMode::AppendToNative,
    }),
    &store,
    &resources,
    mcp_servers,
).await?;
```

Portable restoration resolves the selected immutable history and exact resource
revisions, validates supported content and encoding, and creates a new native
session. The destination can be another provider or slot while retaining the same
application session ID. Selected records must belong to that application session;
cross-session import remains a separate operation.

The selection is frozen at setup. The first `restored.start_recorded_run(...)`
must deliver it through the recorded context path. Configuration can be inspected
or changed with `configuration`, `set_model`, and `set_option` before that turn.
The actual prompt byte limit is checked again with the first task text included.
Unavailable inputs and unsupported roles fail before native session creation;
an oversized first task fails before prompt dispatch.

Once the first run is dispatched, later turns use the retained native session
without replaying that selection. Preparation or storage failure before dispatch
does not consume the pending selection, though a registered run ID cannot be reused.
Abandoning or failing dispatched execution retains the existing retired-session
behavior. This wrapper does not turn an uncertain run into a safe retry.

`into_session` provides the underlying ACP session after the first portable dispatch;
it rejects attempts to bypass that required input. Like other consuming methods,
calling it too early consumes the wrapper. A native-restored session has no pending
portable input and can be extracted immediately. Use recorded runs on the wrapper
to persist restoration evidence.

## Reports and limits

Portable plans require a [context policy](context-policy.md). Exact omissions and
instruction grants are validated when freezing the selection; the grant issuer
must also match the recorded host at first dispatch. Plans using either feature
report data version 2 with preserved policy evidence. A policy-free plan retains
the version 1 report described below.

`report()` returns a versioned JSON setup report. Portable reports list selected
record IDs/revisions, resolved resource IDs/revisions, supplemental instruction
revisions, and the destination application session and slot. They also identify
categories that were not transferred:

- History outside the selection.
- Provider-hidden state and native instruction state.
- Provider configuration, prior tool grants, and skill activation state.

These are scope limits, not an enumeration of unknown provider internals or a count
of omitted records. Destination defaults and caller-supplied MCP configuration
still apply. Required base instructions are rejected rather than converted into
portable user text. Images require the explicit image mode and advertised support.
The existing [context-delivery limits](context.md) still apply.

The first recorded run persists an immutable host-attributed extension with namespace
`agent_bridge`, name `restoration`, and data version `1`, before prompt dispatch.
That record captures the setup choice. It is not a completion receipt. Portable
runs also persist the existing preparation, dispatch-attempt, and response evidence;
their run specs retain the selected manifest. The setup report's
`delivery: pending_first_run` value is a statement about setup, not a mutable status.
Read the input receipts for later evidence.

No new database schema is required. Store mutations and provider setup/dispatch
are still separate operations. Restored context is retained in memory until the
first dispatch; image receipt inspection still requires retaining its resource
archive. There is no background replay or recovery when reopening the database.

## Provider-switch example

```sh
cargo run --features acp,sqlite --example acp_transfer -- \
  /tmp/transfer.sqlite3 /absolute/disposable-workspace \
  opencode acp --to codex-acp
```

For locally pinned adapters, use `node /absolute/path/to/adapter/dist/index.js` as
the executable and arguments. The example runs a source turn, shuts down that
provider, reopens SQLite, selects user and agent messages, and asks a new provider
to recall a unique phrase. It preserves the application session ID, assigns a new
slot, and reopens the restoration report for verification. It dismisses permission
requests and has a 120-second overall timeout. No source native handle is supplied
to the destination.

Verified September 5, 2026 in both directions between OpenCode 1.18.25 and Codex
ACP 1.10.0 with local Codex 0.153.4. Each destination returned the exact selected
phrase, and the restoration report survived SQLite reopen. The transfer used
recorded conversation only, not a native continuation. Claude remains deferred.
