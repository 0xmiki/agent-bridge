use super::*;

fn clock(connection: &Connection, session: &SessionId) -> Result<ChangeCursor, StoreError> {
    session_sequence(connection, session)?;
    let (epoch, position): (String, i64) = connection
        .query_row(
            "SELECT epoch, position FROM agent_bridge_change_clock WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error)?;
    Ok(ChangeCursor {
        epoch,
        session_id: session.clone(),
        position: number(position)?,
    })
}

fn validate(cursor: &ChangeCursor, head: &ChangeCursor) -> Result<(), StoreError> {
    if cursor.epoch != head.epoch
        || cursor.session_id != head.session_id
        || cursor.position > head.position
    {
        return Err(StoreError::InvalidChangeCursor);
    }
    Ok(())
}

impl ChangeStore for SqliteStore {
    fn snapshot_page(
        &self,
        session: &SessionId,
        cursor: Option<&ChangeCursor>,
        after: Option<u64>,
        limit: usize,
    ) -> Result<StatePage, StoreError> {
        if !(1..=1000).contains(&limit) {
            return Err(StoreError::InvalidPageSize);
        }
        if after.is_some() && cursor.is_none() {
            return Err(StoreError::InvalidChangeCursor);
        }
        let mut connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = connection.transaction().map_err(database_error)?;
        let head = clock(&tx, session)?;
        let cursor = match cursor {
            Some(cursor) => {
                validate(cursor, &head)?;
                cursor.clone()
            }
            None => head,
        };
        let after = after
            .map(i64::try_from)
            .transpose()
            .map_err(|_| StoreError::InvalidChangeCursor)?
            .unwrap_or(-1);
        let records = {
            let mut statement = tx.prepare(&format!("SELECT {RECORD_COLUMNS} FROM agent_bridge_records WHERE session_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT ?3")).map_err(database_error)?;
            statement
                .query_map(
                    params![session.as_str(), after, limit as i64],
                    RowData::read,
                )
                .map_err(database_error)?
                .map(|row| Ok(row.map_err(database_error)?.decode()?.current))
                .collect::<Result<Vec<_>, StoreError>>()?
        };
        tx.commit().map_err(database_error)?;
        Ok(StatePage {
            next_after: records.last().map(|r| r.record.sequence),
            page_full: records.len() == limit,
            records,
            cursor,
        })
    }

    fn changes(&self, cursor: &ChangeCursor, limit: usize) -> Result<StatePage, StoreError> {
        if !(1..=1000).contains(&limit) {
            return Err(StoreError::InvalidPageSize);
        }
        let mut connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = connection.transaction().map_err(database_error)?;
        let head = clock(&tx, &cursor.session_id)?;
        validate(cursor, &head)?;
        let columns = RECORD_COLUMNS
            .split(", ")
            .map(|c| format!("r.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let rows = {
            let mut statement = tx.prepare(&format!("SELECT {columns}, c.position FROM agent_bridge_records r JOIN agent_bridge_record_changes c ON c.record_id = r.id WHERE c.session_id = ?1 AND c.position > ?2 ORDER BY c.position LIMIT ?3")).map_err(database_error)?;
            statement
                .query_map(
                    params![
                        cursor.session_id.as_str(),
                        cursor.position as i64,
                        limit as i64
                    ],
                    |row| Ok((RowData::read(row)?, row.get::<_, i64>(11)?)),
                )
                .map_err(database_error)?
                .map(|row| {
                    let (row, position) = row.map_err(database_error)?;
                    Ok((row.decode()?.current, number(position)?))
                })
                .collect::<Result<Vec<_>, StoreError>>()?
        };
        let page_full = rows.len() == limit;
        let position = if page_full {
            rows.last().unwrap().1
        } else {
            head.position
        };
        tx.commit().map_err(database_error)?;
        Ok(StatePage {
            records: rows.into_iter().map(|r| r.0).collect(),
            cursor: ChangeCursor { position, ..head },
            next_after: None,
            page_full,
        })
    }
}
