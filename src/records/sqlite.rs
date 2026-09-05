mod codec;
mod continuation;

use super::*;
use crate::{ContextManifest, InvalidId, SlotId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

const MIGRATIONS: &[&str] = &[
    include_str!("sqlite/migrations/0001_records.sql"),
    include_str!("sqlite/migrations/0002_continuations.sql"),
];
const RECORD_COLUMNS: &str = "id, session_id, run_id, sequence, actor_id, reply_to_id, source_json, payload_json, state, revision, initial_json";

/// Local SQLite records with transactional mutation and versioned JSON payloads.
///
/// Clones share one connection. Separate stores can open the same file. Methods
/// block and can wait up to five seconds for a write lock; keep them off UI threads.
/// This persists records, not provider processes or external execution guarantees.
#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    /// Create or open an application database. Its parent directory must exist.
    /// Only the reserved `agent_bridge_*` tables are migrated; application tables
    /// and the database-wide `user_version` and journal mode are left alone.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::initialize(Connection::open(path).map_err(database_error)?)
    }

    /// SQLite-backed ephemeral storage, useful for running the same contract tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::initialize(Connection::open_in_memory().map_err(database_error)?)
    }

    fn initialize(mut connection: Connection) -> Result<Self, StoreError> {
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(database_error)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(database_error)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(database_error)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn write<T>(
        &self,
        action: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let result = action(&transaction)?;
        transaction.commit().map_err(database_error)?;
        Ok(result)
    }
}

fn database_error(error: rusqlite::Error) -> StoreError {
    if matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    ) {
        StoreError::Busy
    } else {
        StoreError::Database(error.to_string())
    }
}

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database_error)?;
    let has_schema: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'agent_bridge_schema')", [], |r| r.get(0))
        .map_err(database_error)?;
    let version = if has_schema {
        transaction
            .query_row(
                "SELECT version FROM agent_bridge_schema WHERE id = 1",
                [],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| StoreError::CorruptData("schema version row is missing".into()))?
    } else {
        let occupied: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name GLOB 'agent_bridge_*')",
                [],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        if occupied {
            return Err(StoreError::UnversionedSchema);
        }
        0
    };
    if version < 0 || version > MIGRATIONS.len() as i64 || (has_schema && version == 0) {
        return Err(StoreError::UnsupportedSchemaVersion(version));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        transaction.execute_batch(sql).map_err(database_error)?;
        transaction
            .execute(
                "UPDATE agent_bridge_schema SET version = ?1 WHERE id = 1",
                [(index + 1) as i64],
            )
            .map_err(database_error)?;
    }
    // Validate the expected table/column surface even when no migration was needed.
    for query in [
        "SELECT id, next_sequence FROM agent_bridge_sessions LIMIT 0",
        "SELECT id, session_id, slot_id, context_json FROM agent_bridge_runs LIMIT 0",
        "SELECT request_id, response_id FROM agent_bridge_decisions LIMIT 0",
        "SELECT id, session_id, adapter, scope, native_key, predecessor_id, descriptor_json, state, latest FROM agent_bridge_continuations LIMIT 0",
    ] {
        transaction.prepare(query).map_err(database_error)?;
    }
    transaction
        .prepare(&format!(
            "SELECT {RECORD_COLUMNS} FROM agent_bridge_records LIMIT 0"
        ))
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)
}

fn state_name(state: RecordState) -> &'static str {
    match state {
        RecordState::Open => "open",
        RecordState::Complete => "complete",
        RecordState::Interrupted => "interrupted",
    }
}
fn parse_state(state: &str) -> Result<RecordState, StoreError> {
    match state {
        "open" => Ok(RecordState::Open),
        "complete" => Ok(RecordState::Complete),
        "interrupted" => Ok(RecordState::Interrupted),
        _ => Err(StoreError::CorruptData("invalid record state".into())),
    }
}
fn id<T: TryFrom<String, Error = InvalidId>>(value: String) -> Result<T, StoreError> {
    T::try_from(value).map_err(|e| StoreError::CorruptData(e.to_string()))
}
fn number(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value)
        .map_err(|_| StoreError::CorruptData("negative sequence or revision".into()))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialContent {
    payload: Payload,
    state: RecordState,
}

struct Entry {
    initial: Arc<Snapshot>,
    current: Arc<Snapshot>,
}

struct RowData {
    id: String,
    session: String,
    run: Option<String>,
    sequence: i64,
    actor: String,
    reply: Option<String>,
    source: Option<String>,
    payload: String,
    state: String,
    revision: i64,
    initial: Option<String>,
}

impl RowData {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            session: row.get(1)?,
            run: row.get(2)?,
            sequence: row.get(3)?,
            actor: row.get(4)?,
            reply: row.get(5)?,
            source: row.get(6)?,
            payload: row.get(7)?,
            state: row.get(8)?,
            revision: row.get(9)?,
            initial: row.get(10)?,
        })
    }

    fn decode(self) -> Result<Entry, StoreError> {
        let current = Arc::new(Snapshot {
            record: Record {
                id: id(self.id)?,
                session_id: id(self.session)?,
                run_id: self.run.map(id).transpose()?,
                sequence: number(self.sequence)?,
                actor: id(self.actor)?,
                reply_to_id: self.reply.map(id).transpose()?,
                payload: codec::decode(&self.payload)?,
            },
            source: self.source.map(|s| codec::decode(&s)).transpose()?,
            revision: number(self.revision)?,
            state: parse_state(&self.state)?,
        });
        rules::validate(&current.record.payload, current.state)
            .map_err(|e| StoreError::CorruptData(e.to_string()))?;
        let initial = if let Some(initial) = self.initial {
            let content: InitialContent = codec::decode(&initial)?;
            rules::validate(&content.payload, content.state)
                .map_err(|e| StoreError::CorruptData(e.to_string()))?;
            let mut snapshot = (*current).clone();
            snapshot.record.payload = content.payload;
            snapshot.state = content.state;
            snapshot.revision = 0;
            Arc::new(snapshot)
        } else {
            if current.revision != 0 {
                return Err(StoreError::CorruptData(
                    "updated record has no creation snapshot".into(),
                ));
            }
            current.clone()
        };
        Ok(Entry { initial, current })
    }
}

fn entry(connection: &Connection, record: &RecordId) -> Result<Option<Entry>, StoreError> {
    connection
        .query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM agent_bridge_records WHERE id = ?1"),
            [record.as_str()],
            RowData::read,
        )
        .optional()
        .map_err(database_error)?
        .map(RowData::decode)
        .transpose()
}

fn session_sequence(connection: &Connection, session: &SessionId) -> Result<i64, StoreError> {
    connection
        .query_row(
            "SELECT next_sequence FROM agent_bridge_sessions WHERE id = ?1",
            [session.as_str()],
            |r| r.get(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or(StoreError::MissingSession)
}

fn read_run(connection: &Connection, run: &RunId) -> Result<Option<RunSpec>, StoreError> {
    let data = connection
        .query_row(
            "SELECT session_id, slot_id, context_json FROM agent_bridge_runs WHERE id = ?1",
            [run.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    data.map(|(session, slot, context)| {
        Ok(RunSpec {
            id: run.clone(),
            session_id: id(session)?,
            slot_id: id::<SlotId>(slot)?,
            context: codec::decode::<ContextManifest>(&context)?,
            config: (),
        })
    })
    .transpose()
}

fn prepare(connection: &Connection, draft: Draft) -> Result<Arc<Snapshot>, StoreError> {
    rules::validate(&draft.payload, draft.state)?;
    let sequence = session_sequence(connection, &draft.session_id)?;
    if sequence == i64::MAX {
        return Err(StoreError::SequenceExhausted);
    }
    if let Some(run) = &draft.run_id {
        let run = read_run(connection, run)?.ok_or(StoreError::MissingRun)?;
        if run.session_id != draft.session_id {
            return Err(StoreError::WrongSession);
        }
    }
    if let Some(reply) = &draft.reply_to_id
        && entry(connection, reply)?.is_none()
    {
        return Err(StoreError::MissingRecord);
    }
    Ok(Arc::new(Snapshot {
        record: Record {
            id: draft.id,
            session_id: draft.session_id,
            run_id: draft.run_id,
            sequence: number(sequence)?,
            actor: draft.actor,
            reply_to_id: draft.reply_to_id,
            payload: draft.payload,
        },
        source: draft.source,
        revision: 0,
        state: draft.state,
    }))
}

fn insert_row(connection: &Connection, snapshot: &Snapshot) -> Result<(), StoreError> {
    let record = &snapshot.record;
    connection.execute("INSERT INTO agent_bridge_records
        (id, session_id, run_id, sequence, actor_id, reply_to_id, source_json, payload_json, state, revision)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)", params![record.id.as_str(), record.session_id.as_str(),
            record.run_id.as_ref().map(RunId::as_str), record.sequence as i64, record.actor.as_str(),
            record.reply_to_id.as_ref().map(RecordId::as_str), snapshot.source.as_ref().map(codec::encode).transpose()?,
            codec::encode(&record.payload)?, state_name(snapshot.state)]).map_err(database_error)?;
    connection
        .execute(
            "UPDATE agent_bridge_sessions SET next_sequence = next_sequence + 1 WHERE id = ?1",
            [record.session_id.as_str()],
        )
        .map_err(database_error)?;
    Ok(())
}

fn update_row(connection: &Connection, old: &Snapshot, new: &Snapshot) -> Result<(), StoreError> {
    if old == new {
        return Ok(());
    }
    let revision = i64::try_from(new.revision).map_err(|_| StoreError::SequenceExhausted)?;
    let initial = if old.revision == 0 {
        Some(codec::encode(&InitialContent {
            payload: old.record.payload.clone(),
            state: old.state,
        })?)
    } else {
        None
    };
    let changed = connection
        .execute(
            "UPDATE agent_bridge_records SET payload_json = ?1, state = ?2,
        revision = ?3, initial_json = COALESCE(initial_json, ?4) WHERE id = ?5 AND revision = ?6",
            params![
                codec::encode(&new.record.payload)?,
                state_name(new.state),
                revision,
                initial,
                old.record.id.as_str(),
                old.revision as i64
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(StoreError::RevisionConflict);
    }
    Ok(())
}

impl RecordStore for SqliteStore {
    fn create_session(&self, id: SessionId) -> Result<(), StoreError> {
        self.write(|connection| {
            connection
                .execute(
                    "INSERT INTO agent_bridge_sessions (id) VALUES (?1) ON CONFLICT(id) DO NOTHING",
                    [id.as_str()],
                )
                .map_err(database_error)?;
            Ok(())
        })
    }

    fn register_run(&self, spec: RunSpec) -> Result<bool, StoreError> {
        self.write(|connection| {
            if let Some(existing) = read_run(connection, &spec.id)? {
                return if existing == spec { Ok(false) } else { Err(StoreError::IdentityConflict) };
            }
            session_sequence(connection, &spec.session_id)?;
            for id in &spec.context.records {
                let existing = entry(connection, id)?.ok_or(StoreError::MissingRecord)?;
                if !existing.current.state.is_final() { return Err(StoreError::OpenContextRecord); }
            }
            connection.execute("INSERT INTO agent_bridge_runs (id, session_id, slot_id, context_json) VALUES (?1, ?2, ?3, ?4)",
                params![spec.id.as_str(), spec.session_id.as_str(), spec.slot_id.as_str(), codec::encode(&spec.context)?])
                .map_err(database_error)?;
            Ok(true)
        })
    }

    fn insert(&self, draft: Draft) -> Result<Arc<Snapshot>, StoreError> {
        self.write(|connection| {
            if matches!(draft.payload, Payload::Decision { .. }) {
                return Err(StoreError::InvalidDecision);
            }
            if let Some(existing) = entry(connection, &draft.id)? {
                return if rules::same_draft(&existing.initial, &draft) {
                    Ok(existing.current)
                } else {
                    Err(StoreError::IdentityConflict)
                };
            }
            if matches!(draft.payload, Payload::Permission { .. })
                && draft.state != RecordState::Open
            {
                return Err(StoreError::InvalidPayload);
            }
            let snapshot = prepare(connection, draft)?;
            insert_row(connection, &snapshot)?;
            Ok(snapshot)
        })
    }

    fn checkpoint(
        &self,
        id: &RecordId,
        expected_revision: u64,
        payload: Payload,
        state: RecordState,
    ) -> Result<Arc<Snapshot>, StoreError> {
        self.write(|connection| {
            let old = entry(connection, id)?
                .ok_or(StoreError::MissingRecord)?
                .current;
            let new = rules::checkpoint(&old, expected_revision, payload, state)?;
            update_row(connection, &old, &new)?;
            Ok(new)
        })
    }

    fn resolve(
        &self,
        request: &RecordId,
        expected_revision: u64,
        decision: Draft,
    ) -> Result<Arc<Snapshot>, StoreError> {
        self.write(|connection| {
            let response_id: Option<String> = connection
                .query_row(
                    "SELECT response_id FROM agent_bridge_decisions WHERE request_id = ?1",
                    [request.as_str()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(database_error)?;
            if let Some(response_id) = response_id {
                let existing =
                    entry(connection, &id(response_id)?)?.ok_or(StoreError::MissingRecord)?;
                return if rules::same_draft(&existing.initial, &decision) {
                    Ok(existing.current)
                } else {
                    Err(StoreError::AlreadyResolved)
                };
            }
            if entry(connection, &decision.id)?.is_some() {
                return Err(StoreError::IdentityConflict);
            }
            let original = entry(connection, request)?
                .ok_or(StoreError::MissingRecord)?
                .current;
            let resolved = rules::resolve_request(&original, expected_revision, &decision)?;
            let response = prepare(connection, decision)?;
            update_row(connection, &original, &resolved)?;
            insert_row(connection, &response)?;
            connection
                .execute(
                    "INSERT INTO agent_bridge_decisions (request_id, response_id) VALUES (?1, ?2)",
                    params![request.as_str(), response.record.id.as_str()],
                )
                .map_err(database_error)?;
            Ok(response)
        })
    }

    fn get(&self, id: &RecordId) -> Result<Arc<Snapshot>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        entry(&connection, id)?
            .map(|e| e.current)
            .ok_or(StoreError::MissingRecord)
    }

    fn get_run(&self, id: &RunId) -> Result<RunSpec, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        read_run(&connection, id)?.ok_or(StoreError::MissingRun)
    }

    fn list(
        &self,
        session: &SessionId,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Arc<Snapshot>>, StoreError> {
        if !(1..=1000).contains(&limit) {
            return Err(StoreError::InvalidPageSize);
        }
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        session_sequence(&connection, session)?;
        let after = match after {
            None => -1,
            Some(after) => match i64::try_from(after) {
                Ok(after) => after,
                Err(_) => return Ok(vec![]),
            },
        };
        let mut statement = connection
            .prepare(&format!(
                "SELECT {RECORD_COLUMNS} FROM agent_bridge_records
            WHERE session_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT ?3"
            ))
            .map_err(database_error)?;
        let rows = statement
            .query_map(
                params![session.as_str(), after, limit as i64],
                RowData::read,
            )
            .map_err(database_error)?;
        rows.map(|row| Ok(row.map_err(database_error)?.decode()?.current))
            .collect()
    }
}
