//! Read-only restart inventory. Evidence is not proof that another process stopped.
use super::*;

#[derive(Debug, Clone, Serialize)]
pub struct RunRecovery {
    pub id: RunId,
    pub session_id: SessionId,
    pub slot_id: SlotId,
    /// missing_evidence also covers runs recorded by older bridge versions.
    pub dispatch: &'static str,
    pub completion: Option<CompletionReason>,
    pub issues: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InteractionRecovery {
    pub id: RecordId,
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub kind: String,
    pub state: RecordState,
    pub source: Option<SourceRef>,
}

fn limit(value: usize) -> Result<i64, StoreError> {
    if !(1..=1000).contains(&value) {
        return Err(StoreError::CorruptData(
            "discovery limit must be 1..=1000".into(),
        ));
    }
    Ok(value as i64)
}

impl SqliteStore {
    /// Lexicographic keyset pagination. Scan after stopping writers; pages are not
    /// one cross-request snapshot. No IDs from the previous process are required.
    pub fn discover_sessions(
        &self,
        after: Option<&str>,
        count: usize,
    ) -> Result<Vec<SessionId>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let mut query = connection.prepare("SELECT id FROM agent_bridge_sessions WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT ?2").map_err(database_error)?;
        query
            .query_map(params![after, limit(count)?], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .map(|row| id(row.map_err(database_error)?))
            .collect()
    }

    /// Includes completed runs so callers can distinguish completion from missing
    /// evidence. Session-scoped application tools are inventoried separately.
    pub fn discover_runs(
        &self,
        after: Option<&str>,
        count: usize,
    ) -> Result<Vec<RunRecovery>, StoreError> {
        let mut connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = connection.transaction().map_err(database_error)?;
        let mut query = tx.prepare("SELECT id FROM agent_bridge_runs WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT ?2").map_err(database_error)?;
        let ids = query
            .query_map(params![after, limit(count)?], |row| row.get::<_, String>(0))
            .map_err(database_error)?;
        let mut result = Vec::new();
        for run in ids {
            let run = id::<RunId>(run.map_err(database_error)?)?;
            let spec = read_run(&tx, &run)?.ok_or(StoreError::MissingRun)?;
            let mut report = RunRecovery {
                id: run.clone(),
                session_id: spec.session_id,
                slot_id: spec.slot_id,
                dispatch: "missing_evidence",
                completion: None,
                issues: Vec::new(),
            };
            let mut records = tx.prepare("SELECT payload_json, state FROM agent_bridge_records WHERE run_id = ?1 ORDER BY sequence").map_err(database_error)?;
            let mut rows = records.query([run.as_str()]).map_err(database_error)?;
            let (mut open, mut failure, mut contract, mut validation) =
                (false, false, false, false);
            while let Some(row) = rows.next().map_err(database_error)? {
                let payload: Payload =
                    codec::decode(&row.get::<_, String>(0).map_err(database_error)?)?;
                open |= parse_state(&row.get::<_, String>(1).map_err(database_error)?)?
                    == RecordState::Open;
                match payload {
                    Payload::RunFinished { reason } => report.completion = Some(reason),
                    Payload::Failure { .. } => failure = true,
                    Payload::Extension {
                        namespace,
                        name,
                        data,
                    } if namespace == "agent_bridge" => {
                        match name.as_str() {
                            "run_dispatch" => {
                                match (data["version"].as_u64(), data["state"].as_str()) {
                                    (Some(1), Some("prepared")) => {
                                        if report.dispatch == "missing_evidence" {
                                            report.dispatch = "not_dispatched";
                                        }
                                    }
                                    (Some(1), Some("dispatch_attempted")) => {
                                        report.dispatch = "attempted"
                                    }
                                    _ => return Err(StoreError::CorruptData(
                                        "unsupported run dispatch evidence; inspect raw history"
                                            .into(),
                                    )),
                                }
                            }
                            "result_contract" => contract = true,
                            "result_validation"
                                if data["version"] == 1
                                    && matches!(
                                        data["validation"]["status"].as_str(),
                                        Some("valid" | "rejected")
                                    ) =>
                            {
                                validation = true
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            if report.completion.is_none() {
                report.issues.push("completion_unknown");
            }
            if open {
                report.issues.push("open_records");
            }
            if failure {
                report.issues.push("recorded_failure");
            }
            if contract && !validation {
                report.issues.push("validation_missing");
            }
            result.push(report);
        }
        Ok(result)
    }

    /// Requests without durable decisions, permission responses without provider
    /// acknowledgment, and tool attempts without a recorded application result.
    /// These records are evidence for review, never live responders or retry jobs.
    pub fn discover_interactions(
        &self,
        after: Option<&str>,
        count: usize,
    ) -> Result<Vec<InteractionRecovery>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        // ponytail: tool outcome matching scans this session's receipts. Add an
        // indexed invocation projection if large histories exceed the read budget.
        let mut query = connection.prepare(&format!("SELECT {RECORD_COLUMNS} FROM agent_bridge_records AS r
            WHERE (?1 IS NULL OR id > ?1) AND (
                (json_extract(payload_json, '$.data.type') IN ('permission','question') AND NOT EXISTS
                    (SELECT 1 FROM agent_bridge_decisions d WHERE d.request_id = r.id))
                OR json_extract(payload_json, '$.data.type') = 'decision'
                OR (json_extract(payload_json, '$.data.type') = 'tool' AND state != 'complete')
                OR (json_extract(payload_json, '$.data.data.namespace') = 'agent_bridge'
                    AND json_extract(payload_json, '$.data.data.name') = 'tool_invocation'
                    AND json_extract(payload_json, '$.data.data.data.state') = 'dispatch_attempted'
                    AND NOT EXISTS (SELECT 1 FROM agent_bridge_records outcome
                        WHERE outcome.session_id = r.session_id
                        AND json_extract(outcome.payload_json, '$.data.data.namespace') = 'agent_bridge'
                        AND json_extract(outcome.payload_json, '$.data.data.name') = 'tool_invocation'
                        AND json_extract(outcome.payload_json, '$.data.data.data.version') = 1
                        AND json_extract(outcome.payload_json, '$.data.data.data.invocation_id') = json_extract(r.payload_json, '$.data.data.data.invocation_id')
                        AND json_extract(outcome.payload_json, '$.data.data.data.state') = 'returned'))
            ) ORDER BY id LIMIT ?2")).map_err(database_error)?;
        query
            .query_map(params![after, limit(count)?], RowData::read)
            .map_err(database_error)?
            .map(|row| {
                let entry = row.map_err(database_error)?.decode()?;
                let record = &entry.current.record;
                let kind = match &record.payload {
                    Payload::Permission { .. } => "permission_without_decision",
                    Payload::Question(_) => "question_without_answer",
                    Payload::Decision { .. } => "permission_delivery_unacknowledged",
                    Payload::Tool(_) => "provider_tool_incomplete",
                    _ => "application_tool_outcome_unknown",
                };
                Ok(InteractionRecovery {
                    id: record.id.clone(),
                    session_id: record.session_id.clone(),
                    run_id: record.run_id.clone(),
                    kind: kind.into(),
                    state: entry.current.state,
                    source: entry.current.source.clone(),
                })
            })
            .collect()
    }

    pub fn discover_continuations(
        &self,
        after: Option<&str>,
        count: usize,
    ) -> Result<Vec<Arc<ContinuationRecord>>, StoreError> {
        let mut connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = connection.transaction().map_err(database_error)?;
        let mut query = tx.prepare("SELECT id FROM agent_bridge_continuations WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT ?2").map_err(database_error)?;
        query
            .query_map(params![after, limit(count)?], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .map(|row| {
                continuation::read(&tx, &id(row.map_err(database_error)?)?)?
                    .ok_or(StoreError::MissingContinuation)
            })
            .collect()
    }
}
