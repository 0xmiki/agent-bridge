# SQLite storage

The `sqlite` feature implements the existing `RecordStore` interface with local
transactions. It uses bundled SQLite through rusqlite, so an application does not
need a separate SQLite installation. The Rust development shell already includes
the C compiler needed to build it. ACP remains an independent optional feature.

## Usage

```rust
use agent_bridge::records::{RecordStore, SqliteStore};
use agent_bridge::SessionId;

let store = SqliteStore::open("application.sqlite3")?;
store.create_session(SessionId::new("conversation-1")?)?;
let history = store.list(&SessionId::new("conversation-1")?, None, 100)?;
```

Pass `&store` to `session.start_recorded_run` exactly as with `MemoryStore`. Reopening
the same file restores saved records, context references, run registrations, and
decision history. `SqliteStore::open_in_memory()` exercises the SQL implementation
without creating a file.

Opening a path creates the database if absent. Its parent directory must exist.
Clones share one connection; separate handles can open the same file. Operations
are synchronous and may wait up to five seconds for a write lock, so keep them off
UI threads and account for blocking in async hosts. Lock contention returns `Busy`.

## Schema version 4

| Table | Responsibility |
| --- | --- |
| `agent_bridge_schema` | This package's migration version |
| `agent_bridge_sessions` | Session identity and next record sequence |
| `agent_bridge_runs` | Execution identity, session, slot, context, configuration, and continuation origin |
| `agent_bridge_records` | Attributed payloads, ordering, revisions, and original-insert data |
| `agent_bridge_decisions` | One decision record for each resolved permission request |
| `agent_bridge_continuations` | Single-use provider handoffs and successor chains |
| `agent_bridge_resource_versions` | Immutable resource ID/revision, media type, and blob digest |
| `agent_bridge_resource_blobs` | One binary blob per SHA-256 digest |

Version 1 adds records in
[0001_records.sql](../src/records/sqlite/migrations/0001_records.sql). Version 2 adds
continuations in
[0002_continuations.sql](../src/records/sqlite/migrations/0002_continuations.sql).
Opening a version 1 database upgrades it transactionally and preserves its records.
Version 3 adds run configuration and continuation links in
[0003_run_configuration.sql](../src/records/sqlite/migrations/0003_run_configuration.sql).
Older runs keep unknown configuration and a null continuation link. Record JSON
format remains version 1.
Version 4 adds immutable resource storage in
[0004_resources.sql](../src/records/sqlite/migrations/0004_resources.sql).
Slots remain
host configuration; this record store persists their references, not executables,
or credentials. Run rows now preserve configuration reports independently of slot
defaults, without claiming that reports attest the remote model's actual identity.

Tables and indexes use the reserved `agent_bridge_` prefix. Application tables,
`PRAGMA user_version`, and journal mode are not changed. The adapter uses its own
single-row version table. It enables foreign keys and FULL synchronous mode on its
connection. Do not modify managed tables directly; application extensions can use
separate tables referencing their IDs.

Migrations run in one transaction. The adapter rejects a newer schema version,
missing expected columns, or reserved tables without a version marker. It does not
guess their meaning, reset a database, or perform an implicit downgrade.

## Versioned payloads

Payloads, context manifests, source references, and original content use JSON
documents with an explicit format version:

```json
{
  "version": 1,
  "data": {
    "type": "message",
    "data": {
      "kind": "agent",
      "message": { "content": [{ "type": "text", "data": "Hello" }] }
    }
  }
}
```

The decoder rejects unsupported document versions and malformed content. It does
not skip unreadable records when listing history. Typed IDs retain their validation
when deserialized. Namespaced extension payloads preserve their arbitrary JSON.

An untouched record stores its content once. The first checkpoint retains its
creation payload/state in `initial_json` for idempotent insertion retries; later
checkpoints replace current content without retaining every delta. Identity columns
stay separate and are not duplicated in that creation document.

SQL sequences and revisions are nonnegative signed 64-bit integers. The API detects
exhaustion rather than overflowing. History uses an indexed `(session_id, sequence)`
query with an exclusive cursor and a page limit of 1 through 1000.

## Atomicity and restart boundaries

Each mutation uses an IMMEDIATE transaction. Sequence allocation, record insertion,
checkpoint validation, and permission resolution remain consistent across independent
connections. A failed resolution rolls back the request revision, response record,
decision link, and sequence increment together.

Disk persistence does not establish that a provider executed or received an action.
Reopening does not resume agents, replay prompts, answer permissions, or finalize open
records. An open record remains open; it may have been left by a crash or still belong
to another process. Recorded run IDs remain registered, preventing accidental reuse
through the recorded-run API.

The whole transcript is not committed in one transaction. Each store operation is
atomic; a crash between operations can leave partial progress. Durable execution
intent, uncertain provider outcomes, and decision delivery recovery need further work.

## Verification and future changes

Memory and SQLite run the same storage-contract tests. Additional tests cover reopen,
payload round trips, coexistence with application tables, unsupported versions,
corruption, rollback after an injected SQL failure, and independent-connection races.
ACP fixture runs also read transcripts and resume a saved continuation after SQLite
reopens.

The public API can keep improving, but SQL schema versions 1 through 4 and JSON format 1
are now compatibility obligations. New schema steps belong in migrations. New JSON
formats need an explicit upgrade or a retained old-version decoder. Async access,
configurable lock timeouts,
resource deletion/garbage collection, and uncertain-outcome reconciliation remain open design questions.

## Resource retention

`SqliteStore` also implements the independent `ResourceStore` and `ResourceArchive`
traits from `agent_bridge::context`. Import `ResourceArchive` to call `store.put(resource)`.
If both read traits are imported, use `ResourceStore::get(&store, &reference)` to
distinguish resource lookup from `RecordStore::get`.

Identical writes at one resource revision are idempotent. Different bytes or media
types at that revision are rejected. Different revisions or resource IDs can share
one blob when their bytes match. Blob insertion and revision registration use one
IMMEDIATE transaction; a failed revision insert leaves no new orphan blob. Separate
connections serialize competing writes, so a revision has one immutable winner.
Reads verify the blob digest and report missing or mismatched blobs as corruption.

Resource revision labels remain application-assigned; the blob digest is a separate
integrity and storage-sharing mechanism. This store has no automatic deletion or
garbage collection. It does not authenticate callers or enforce per-session resource
access. Use an appropriately scoped store in applications that need that boundary.
Resources are loaded into memory on read; applications can use a custom store for
other retention or allocation requirements.

References: [SQLite transactions](https://www.sqlite.org/lang_transaction.html) and
[rusqlite](https://docs.rs/rusqlite/0.40.2/rusqlite/).
