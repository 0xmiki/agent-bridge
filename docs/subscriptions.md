# Run observation

The experimental Bun client separates observing a run from controlling it. Closing
a view, abandoning an iterator, or falling behind does not cancel agent work.
The owner uses `run.cancel()` explicitly and reads the actual outcome through
`run.completed`.

## API and limits

`run.events` is the default subscription, attached when the run is created.
`run.subscribe()` adds an observer of future events. Each subscription has one
async iterator and its own queue. A subscriber attached after completion receives
only the terminal event; saved history supplies earlier content.

```ts
const run = session.run("Summarize the project.");
const view = run.subscribe({ maxEvents: 64, maxBytes: 256 * 1024 });
try {
  for await (const event of view) render(event);
} catch (error) {
  if (error instanceof SubscriptionLaggedError) await state.sync(host);
  else throw error;
} finally {
  view.close();
}
const outcome = await run.completed;
```

Import `SubscriptionLaggedError` from `host/client.ts`. There are at most eight
active subscriptions per run, including the default. Each defaults to 128 queued
events and 1 MiB of serialized UTF-8 event bytes. Options may lower these limits;
invalid or higher limits are rejected. Byte accounting limits queued payload size,
not total JavaScript heap usage. Events delivered directly to a waiting iterator
are not queued and still obey the host's frame-size limit.

When a queue fills, only that subscription fails with code `subscriber_lagged`.
Its queued references are cleared and its active slot is released. The client
continues draining the shared wire, other subscribers keep receiving events, and
the run's completion promise remains independent. Use saved state to recover;
this is not replay of missed stream events.

`close()` is idempotent. Breaking out of a `for await` loop also closes the
subscription. Completion and errors release active subscriber slots. Delivered
events and permission options are frozen to prevent one observer from changing
what another observer sees.

## Recovering interactions

A slow observer may miss a permission request. Attach a replacement subscription
first, then call `await run.pendingPermissions()` to query the host's currently
pending requests. Merge the snapshot with incoming events by `permission_id`;
the same request may appear in both. Answer through `run.respond()` using its live
token. The host still rejects invalid, duplicate, or stale responses.

The snapshot is read-only and does not repeat prompts, resolve requests, or recreate
permissions from history. A terminal or superseded run has no pending requests.
This supports reattaching within the current host. After a crash, use [restart
discovery](host-recovery.md) to inspect saved evidence, not to recreate live responders.

## Scope

This isolates observers inside the Bun client. A malformed protocol frame, a lost
stdio connection, or host output exhaustion still affects the shared host. It does
not establish independent network clients or provider-output scheduling fairness.

Fixture tests cover an unread run alongside another active run, fast and slow
observers of the same run, independent byte/event limits, exact saved text after
lag, unsubscribe/admission behavior, immutable permission options, and recovery of
a missed permission without prompt replay or automatic cancellation.
