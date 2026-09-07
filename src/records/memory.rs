use super::rules::{same_draft, validate};
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
    execution_relations: HashMap<RunId, Arc<crate::execution::ExecutionRelation>>,
    continuations: continuation::Registry,
    sessions: HashMap<SessionId, Vec<RecordId>>,
    runs: HashMap<RunId, RunSpec>,
    records: HashMap<RecordId, Entry>,
    decisions: HashMap<RecordId, RecordId>,
}

impl crate::execution::ExecutionStore for MemoryStore {
    fn link_execution(
        &self,
        relation: crate::execution::ExecutionRelation,
    ) -> Result<Arc<crate::execution::ExecutionRelation>, StoreError> {
        let mut state = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        if let Some(existing) = state.execution_relations.get(&relation.child) {
            return if **existing == relation {
                Ok(existing.clone())
            } else {
                Err(StoreError::ExecutionRelationConflict)
            };
        }
        let parent = state
            .runs
            .get(&relation.parent)
            .ok_or(StoreError::MissingRun)?;
        let child = state
            .runs
            .get(&relation.child)
            .ok_or(StoreError::MissingRun)?;
        relation.validate(parent, child)?;
        if state
            .execution_relations
            .get(&relation.parent)
            .is_some_and(|edge| edge.child_authority != relation.parent_authority)
            || state.execution_relations.values().any(|edge| {
                edge.parent == relation.child && edge.parent_authority != relation.child_authority
            })
        {
            return Err(StoreError::InvalidExecutionRelation);
        }
        for id in &child.context.records {
            if state
                .records
                .get(id)
                .ok_or(StoreError::MissingRecord)?
                .current
                .record
                .session_id
                != child.session_id
            {
                return Err(StoreError::WrongSession);
            }
        }
        let mut ancestor = relation.parent.clone();
        let mut visited = std::collections::HashSet::new();
        loop {
            if ancestor == relation.child || !visited.insert(ancestor.clone()) {
                return Err(StoreError::ExecutionCycle);
            }
            match state.execution_relations.get(&ancestor) {
                Some(edge) => ancestor = edge.parent.clone(),
                None => break,
            }
        }
        let relation = Arc::new(relation);
        state
            .execution_relations
            .insert(relation.child.clone(), relation.clone());
        Ok(relation)
    }
    fn execution_parent(
        &self,
        child: &RunId,
    ) -> Result<Option<Arc<crate::execution::ExecutionRelation>>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| StoreError::Poisoned)?
            .execution_relations
            .get(child)
            .cloned())
    }
    fn execution_children(
        &self,
        parent: &RunId,
    ) -> Result<Vec<Arc<crate::execution::ExecutionRelation>>, StoreError> {
        let state = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        let mut children: Vec<_> = state
            .execution_relations
            .values()
            .filter(|edge| &edge.parent == parent)
            .cloned()
            .collect();
        children.sort_by(|a, b| a.child.as_str().cmp(b.child.as_str()));
        Ok(children)
    }
}

impl ContinuationStore for MemoryStore {
    fn save_continuation(
        &self,
        value: Continuation,
    ) -> Result<Arc<ContinuationRecord>, StoreError> {
        let mut state = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        if !state.sessions.contains_key(&value.session_id) {
            return Err(StoreError::MissingSession);
        }
        state.continuations.save(value)
    }
    fn get_continuation(
        &self,
        id: &crate::ContinuationId,
    ) -> Result<Arc<ContinuationRecord>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Poisoned)?
            .continuations
            .get(id)
    }
    fn claim_continuation(
        &self,
        id: &crate::ContinuationId,
    ) -> Result<Arc<ContinuationRecord>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Poisoned)?
            .continuations
            .claim(id)
    }
}

struct Entry {
    // Shared initially; retaining the creation snapshot permits idempotent retries
    // without keeping every token checkpoint or copying an unchanged payload.
    initial: Arc<Snapshot>,
    current: Arc<Snapshot>,
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
        if let Some(id) = &spec.continuation {
            let saved = state.continuations.get(id)?;
            if saved.state != ContinuationState::Claimed
                || !saved.latest
                || saved.continuation.session_id != spec.session_id
                || saved.continuation.slot_id != spec.slot_id
            {
                return Err(StoreError::InvalidContinuation);
            }
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
        if matches!(
            draft.payload,
            Payload::Decision { .. } | Payload::Answer { .. }
        ) {
            return Err(StoreError::InvalidDecision);
        }
        if let Some(entry) = state.records.get(&draft.id) {
            return if same_draft(&entry.initial, &draft) {
                Ok(entry.current.clone())
            } else {
                Err(StoreError::IdentityConflict)
            };
        }
        if matches!(
            draft.payload,
            Payload::Permission { .. } | Payload::Question(_)
        ) && draft.state != RecordState::Open
        {
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
        let snapshot = rules::checkpoint(&entry.current, expected_revision, payload, next)?;
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
        let resolved = rules::resolve_request(&original, expected_revision, &decision)?;
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
