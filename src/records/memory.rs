use super::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Shared, process-local records. Clones refer to the same store. No disk durability.
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<SessionId, Vec<RecordId>>,
    runs: HashMap<RunId, RunSpec>,
    records: HashMap<RecordId, Entry>,
    decisions: HashMap<RecordId, RecordId>,
}

struct Entry {
    // Shared initially; retaining the creation snapshot permits idempotent retries
    // without keeping every token checkpoint or copying an unchanged payload.
    initial: Arc<Snapshot>,
    current: Arc<Snapshot>,
}

fn same_draft(snapshot: &Snapshot, draft: &Draft) -> bool {
    let record = &snapshot.record;
    record.id == draft.id
        && record.session_id == draft.session_id
        && record.run_id == draft.run_id
        && record.actor == draft.actor
        && record.reply_to_id == draft.reply_to_id
        && record.payload == draft.payload
        && snapshot.source == draft.source
        && snapshot.state == draft.state
}

fn validate(payload: &Payload, state: RecordState) -> Result<(), StoreError> {
    if let Payload::Permission { options, .. } = payload {
        let mut ids = std::collections::HashSet::new();
        if options
            .iter()
            .any(|o| o.id.trim().is_empty() || !ids.insert(&o.id))
        {
            return Err(StoreError::InvalidPayload);
        }
    }
    if matches!(payload, Payload::Decision { .. }) && state != RecordState::Complete {
        return Err(StoreError::InvalidPayload);
    }
    Ok(())
}

impl State {
    fn prepare(&self, draft: Draft) -> Result<Arc<Snapshot>, StoreError> {
        validate(&draft.payload, draft.state)?;
        let session = self
            .sessions
            .get(&draft.session_id)
            .ok_or(StoreError::MissingSession)?;
        if let Some(run) = &draft.run_id {
            let run = self.runs.get(run).ok_or(StoreError::MissingRun)?;
            if run.session_id != draft.session_id {
                return Err(StoreError::WrongSession);
            }
        }
        if let Some(reply) = &draft.reply_to_id
            && !self.records.contains_key(reply)
        {
            return Err(StoreError::MissingRecord);
        }
        let sequence = u64::try_from(session.len()).map_err(|_| StoreError::SequenceExhausted)?;
        Ok(Arc::new(Snapshot {
            record: Record {
                id: draft.id,
                session_id: draft.session_id,
                run_id: draft.run_id,
                sequence,
                actor: draft.actor,
                reply_to_id: draft.reply_to_id,
                payload: draft.payload,
            },
            source: draft.source,
            revision: 0,
            state: draft.state,
        }))
    }

    fn commit(&mut self, snapshot: Arc<Snapshot>) {
        self.sessions
            .get_mut(&snapshot.record.session_id)
            .unwrap()
            .push(snapshot.record.id.clone());
        self.records.insert(
            snapshot.record.id.clone(),
            Entry {
                initial: snapshot.clone(),
                current: snapshot,
            },
        );
    }
}

impl RecordStore for MemoryStore {
    fn create_session(&self, id: SessionId) -> Result<(), StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Poisoned)?
            .sessions
            .entry(id)
            .or_default();
        Ok(())
    }

    fn register_run(&self, spec: RunSpec) -> Result<bool, StoreError> {
        let mut state = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        if let Some(existing) = state.runs.get(&spec.id) {
            return if existing == &spec {
                Ok(false)
            } else {
                Err(StoreError::IdentityConflict)
            };
        }
        if !state.sessions.contains_key(&spec.session_id) {
            return Err(StoreError::MissingSession);
        }
        for id in &spec.context.records {
            let entry = state.records.get(id).ok_or(StoreError::MissingRecord)?;
            if !entry.current.state.is_final() {
                return Err(StoreError::OpenContextRecord);
            }
        }
        state.runs.insert(spec.id.clone(), spec);
        Ok(true)
    }

    fn insert(&self, draft: Draft) -> Result<Arc<Snapshot>, StoreError> {
        let mut state = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        if matches!(draft.payload, Payload::Decision { .. }) {
            return Err(StoreError::InvalidDecision);
        }
        if let Some(entry) = state.records.get(&draft.id) {
            return if same_draft(&entry.initial, &draft) {
                Ok(entry.current.clone())
            } else {
                Err(StoreError::IdentityConflict)
            };
        }
        if matches!(draft.payload, Payload::Permission { .. }) && draft.state != RecordState::Open {
            return Err(StoreError::InvalidPayload);
        }
        let snapshot = state.prepare(draft)?;
        state.commit(snapshot.clone());
        Ok(snapshot)
    }

    fn checkpoint(
        &self,
        id: &RecordId,
        expected_revision: u64,
        payload: Payload,
        next: RecordState,
    ) -> Result<Arc<Snapshot>, StoreError> {
        let mut state = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        let entry = state.records.get_mut(id).ok_or(StoreError::MissingRecord)?;
        let current = &entry.current;
        // An exact retry of the previous successful write is harmless.
        if current.record.payload == payload
            && current.state == next
            && (current.revision == expected_revision
                || Some(current.revision) == expected_revision.checked_add(1))
        {
            return Ok(current.clone());
        }
        if current.revision != expected_revision {
            return Err(StoreError::RevisionConflict);
        }
        if current.state.is_final() {
            return Err(StoreError::Finalized);
        }
        if std::mem::discriminant(&current.record.payload) != std::mem::discriminant(&payload) {
            return Err(StoreError::InvalidPayload);
        }
        if let (Payload::Message { kind: old, .. }, Payload::Message { kind: new, .. }) =
            (&current.record.payload, &payload)
            && old != new
        {
            return Err(StoreError::InvalidPayload);
        }
        if matches!(payload, Payload::Permission { .. })
            && (payload != current.record.payload || next == RecordState::Complete)
        {
            return Err(StoreError::InvalidDecision);
        }
        validate(&payload, next)?;
        let revision = current
            .revision
            .checked_add(1)
            .ok_or(StoreError::SequenceExhausted)?;
        let mut snapshot = (**current).clone();
        snapshot.record.payload = payload;
        snapshot.revision = revision;
        snapshot.state = next;
        let snapshot = Arc::new(snapshot);
        entry.current = snapshot.clone();
        Ok(snapshot)
    }

    fn resolve(
        &self,
        request: &RecordId,
        expected_revision: u64,
        decision: Draft,
    ) -> Result<Arc<Snapshot>, StoreError> {
        let mut state = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        if let Some(existing_id) = state.decisions.get(request) {
            let entry = state.records.get(existing_id).unwrap();
            return if same_draft(&entry.initial, &decision) {
                Ok(entry.current.clone())
            } else {
                Err(StoreError::AlreadyResolved)
            };
        }
        if state.records.contains_key(&decision.id) {
            return Err(StoreError::IdentityConflict);
        }
        let original = state
            .records
            .get(request)
            .ok_or(StoreError::MissingRecord)?
            .current
            .clone();
        if original.revision != expected_revision {
            return Err(StoreError::RevisionConflict);
        }
        if original.state != RecordState::Open {
            return Err(StoreError::Finalized);
        }
        let Payload::Permission { options, .. } = &original.record.payload else {
            return Err(StoreError::InvalidDecision);
        };
        let Payload::Decision { outcome, .. } = &decision.payload else {
            return Err(StoreError::InvalidDecision);
        };
        if decision.reply_to_id.as_ref() != Some(request)
            || decision.session_id != original.record.session_id
            || decision.run_id != original.record.run_id
        {
            return Err(StoreError::InvalidDecision);
        }
        if let PermissionOutcome::Selected(id) = outcome
            && !options.iter().any(|option| &option.id == id)
        {
            return Err(StoreError::InvalidDecision);
        }
        let mut resolved = (*original).clone();
        resolved.state = RecordState::Complete;
        resolved.revision = resolved
            .revision
            .checked_add(1)
            .ok_or(StoreError::SequenceExhausted)?;
        let snapshot = state.prepare(decision)?;
        state
            .decisions
            .insert(request.clone(), snapshot.record.id.clone());
        state.records.get_mut(request).unwrap().current = Arc::new(resolved);
        state.commit(snapshot.clone());
        Ok(snapshot)
    }

    fn get(&self, id: &RecordId) -> Result<Arc<Snapshot>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Poisoned)?
            .records
            .get(id)
            .map(|entry| entry.current.clone())
            .ok_or(StoreError::MissingRecord)
    }

    fn get_run(&self, id: &RunId) -> Result<RunSpec, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Poisoned)?
            .runs
            .get(id)
            .cloned()
            .ok_or(StoreError::MissingRun)
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
        let state = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        let ids = state
            .sessions
            .get(session)
            .ok_or(StoreError::MissingSession)?;
        let start = match after {
            None => 0,
            Some(cursor) => cursor
                .checked_add(1)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(usize::MAX),
        };
        Ok(ids
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take(limit)
            .map(|id| state.records[id].current.clone())
            .collect())
    }
}
