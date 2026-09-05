use super::*;
use crate::ContinuationId;

pub(super) fn read(
    connection: &Connection,
    id: &ContinuationId,
) -> Result<Option<Arc<ContinuationRecord>>, StoreError> {
    let row = connection.query_row("SELECT descriptor_json, state, latest, session_id, adapter, scope, native_key, predecessor_id
        FROM agent_bridge_continuations WHERE id = ?1", [id.as_str()], |row| Ok((
            row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, bool>(2)?, row.get::<_, String>(3)?,
            row.get::<_, String>(4)?, row.get::<_, String>(5)?, row.get::<_, String>(6)?, row.get::<_, Option<String>>(7)?
        ))).optional().map_err(database_error)?;
    row.map(
        |(data, state, latest, session, adapter, scope, key, predecessor)| {
            let value: Continuation = codec::decode(&data)?;
            super::super::continuation::validate(&value)?;
            if &value.id != id
                || value.session_id.as_str() != session
                || value.adapter != adapter
                || value.scope != scope
                || value.native_key != key
                || value.predecessor.as_ref().map(|id| id.as_str()) != predecessor.as_deref()
            {
                return Err(StoreError::CorruptData(
                    "continuation descriptor does not match its indexed identity".into(),
                ));
            }
            let state = match state.as_str() {
                "available" => ContinuationState::Available,
                "claimed" => ContinuationState::Claimed,
                _ => return Err(StoreError::CorruptData("invalid continuation state".into())),
            };
            Ok(Arc::new(ContinuationRecord {
                continuation: value,
                state,
                latest,
            }))
        },
    )
    .transpose()
}

impl ContinuationStore for SqliteStore {
    fn save_continuation(
        &self,
        value: Continuation,
    ) -> Result<Arc<ContinuationRecord>, StoreError> {
        self.write(|connection| {
            session_sequence(connection, &value.session_id)?;
            super::super::continuation::validate(&value)?;
            if let Some(existing) = read(connection, &value.id)? {
                return if existing.continuation == value { Ok(existing) } else { Err(StoreError::IdentityConflict) };
            }
            let previous_id: Option<String> = connection.query_row("SELECT id FROM agent_bridge_continuations
                WHERE adapter = ?1 AND scope = ?2 AND native_key = ?3 AND latest = 1",
                params![value.adapter, value.scope, value.native_key], |r| r.get(0)).optional().map_err(database_error)?;
            let previous = previous_id.map(|id| read(connection, &super::id(id)?)).transpose()?.flatten();
            super::super::continuation::validate_successor(&value, previous.as_deref())?;
            if let Some(previous) = previous {
                connection.execute("UPDATE agent_bridge_continuations SET latest = 0 WHERE id = ?1", [previous.continuation.id.as_str()]).map_err(database_error)?;
            }
            connection.execute("INSERT INTO agent_bridge_continuations
                (id, session_id, adapter, scope, native_key, predecessor_id, descriptor_json, state, latest)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'available', 1)", params![value.id.as_str(), value.session_id.as_str(),
                    value.adapter, value.scope, value.native_key, value.predecessor.as_ref().map(ContinuationId::as_str), codec::encode(&value)?]).map_err(database_error)?;
            Ok(Arc::new(ContinuationRecord { continuation: value, state: ContinuationState::Available, latest: true }))
        })
    }

    fn get_continuation(&self, id: &ContinuationId) -> Result<Arc<ContinuationRecord>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        read(&connection, id)?.ok_or(StoreError::MissingContinuation)
    }

    fn claim_continuation(
        &self,
        id: &ContinuationId,
    ) -> Result<Arc<ContinuationRecord>, StoreError> {
        self.write(|connection| {
            let record = read(connection, id)?.ok_or(StoreError::MissingContinuation)?;
            if record.state != ContinuationState::Available || !record.latest {
                return Err(StoreError::ContinuationClaimed);
            }
            connection
                .execute(
                    "UPDATE agent_bridge_continuations SET state = 'claimed' WHERE id = ?1",
                    [id.as_str()],
                )
                .map_err(database_error)?;
            let mut record = (*record).clone();
            record.state = ContinuationState::Claimed;
            Ok(Arc::new(record))
        })
    }
}
