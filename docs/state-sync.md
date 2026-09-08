# Record state synchronization

This is the first SQLite and host synchronization contract. It can evolve as real
applications use it. `RecordStore::list` remains creation-order history pagination.
The optional `ChangeStore` trait adds `snapshot_page` and `changes`; SQLite currently
implements it. Other stores can implement the contract without changing RecordStore.

## Contract

Each inserted or checkpointed record receives a new change position in the same
transaction as the record write. Schema 7 keeps one index entry per record, not copies
of every streamed transcript revision. Multiple updates may coalesce into one upsert.
This feed describes current state; it is not an event log for executing effects.
There are no record deletes or tombstones in the current storage API.

Host read requests open current-schema databases read-only. They do not migrate or
compete for a reserved write lock. An exclusive database lock can still fail a refresh;
the client keeps its last committed projection/cursor and may retry that read later.
Initialize older databases through a writable owner before synchronizing them.

A cursor contains a database epoch, session ID, and position. Host positions use
decimal strings. Treat cursors as values and persist them with the corresponding
projection. Foreign databases/sessions and future positions are rejected. Restoring
an older backup requires discarding the old projection/cursor and scanning again.

1. Call `snapshot_page(session, None, None, limit)`.
2. Keep its cursor for every subsequent snapshot page; advance only `next_after`.
3. Read `changes` starting from that original cursor, following each returned cursor.
4. Upsert by record ID, accepting only newer revisions. Repeated rows are harmless.
5. Save the resulting projection and cursor together. On failure, retry from the last
   committed pair.

Snapshot pagination reads current records, not a frozen historical image. A later
page may already contain a change that the feed returns again. Keeping the original
cursor and applying revisions monotonically prevents gaps and regression. Each page's
clock and records are read in one SQLite read transaction. Changes are ordered by
change position; presentation remains ordered by record creation sequence.

Migration from schema 6 indexes existing records at their current revisions. It does
not invent earlier revision history. The clock and index roll back with failed writes.

## Application use

```ts
import { SessionState } from "../host/state";

const state = new SessionState(databasePath, sessionId, savedCheckpoint);
await state.sync(host);
for (const item of state.items) {
  if (item.kind === "message") renderMessage(item.role, item.text);
}
await saveCheckpoint(state.checkpoint());
```

`SessionState` commits a synchronization only when all pages succeed. It rejects
overlapping sync calls and limits one synchronization to 1,000 pages. Applications
choose when to poll and can use the page API directly for larger datasets. No
background subscriber queue grows while the UI is disconnected.

If a live [run observer](subscriptions.md) lags, synchronize this saved state and
attach a new observer. Use `run.pendingPermissions()` for current interaction tokens;
portable history does not itself recreate live permission handles.

The initial typed readers expose message roles/text, tool titles/statuses, permission
options, failures, and completion reasons. Raw records retain resources, decisions,
questions, extensions, and receipt details. More typed receipt readers and independent
subscriber scheduling remain open; this increment does not settle the whole public API.

Tests cover updates to old records, overlapping pages, coalescing, exact reopened
state, cursor misuse, schema-6 backfill, rollback, and failure without cursor advancement.
