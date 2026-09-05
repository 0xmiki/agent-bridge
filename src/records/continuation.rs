use super::{RecordStore, StoreError};
use crate::{ContinuationId, SessionId, SlotId};
use std::{collections::HashMap, sync::Arc};

/// An immutable handoff descriptor. It is a native context locator, not a
/// transcript snapshot or proof that another client has not changed that context.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct Continuation {
    pub id: ContinuationId,
    pub session_id: SessionId,
    pub slot_id: SlotId,
    pub adapter: String,
    /// Application-owned account/profile/environment namespace. Never a credential.
    pub scope: String,
    pub native_key: String,
    pub predecessor: Option<ContinuationId>,
    /// Adapter-versioned, non-secret native metadata.
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationState {
    Available,
    Claimed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationRecord {
    pub continuation: Continuation,
    pub state: ContinuationState,
    pub latest: bool,
}

/// Explicit, single-use handoffs. Claims never expire automatically: a crashed
/// claimant may already have resumed or executed work. Recovery is separate work.
pub trait ContinuationStore: RecordStore {
    fn save_continuation(
        &self,
        continuation: Continuation,
    ) -> Result<Arc<ContinuationRecord>, StoreError>;
    fn get_continuation(&self, id: &ContinuationId) -> Result<Arc<ContinuationRecord>, StoreError>;
    fn claim_continuation(
        &self,
        id: &ContinuationId,
    ) -> Result<Arc<ContinuationRecord>, StoreError>;
}

pub(super) fn validate(value: &Continuation) -> Result<(), StoreError> {
    if value.adapter.trim().is_empty()
        || value.scope.trim().is_empty()
        || value.native_key.trim().is_empty()
        || value.predecessor.as_ref() == Some(&value.id)
    {
        return Err(StoreError::InvalidContinuation);
    }
    Ok(())
}

pub(super) fn validate_successor(
    value: &Continuation,
    latest: Option<&ContinuationRecord>,
) -> Result<(), StoreError> {
    match (latest, &value.predecessor) {
        (None, None) => Ok(()),
        (Some(previous), Some(id))
            if previous.latest
                && previous.state == ContinuationState::Claimed
                && &previous.continuation.id == id
                && previous.continuation.session_id == value.session_id
                && previous.continuation.slot_id == value.slot_id =>
        {
            Ok(())
        }
        _ => Err(StoreError::ContinuationConflict),
    }
}

#[derive(Default)]
pub(super) struct Registry {
    records: HashMap<ContinuationId, Arc<ContinuationRecord>>,
    latest: HashMap<(String, String, String), ContinuationId>,
}

impl Registry {
    pub(super) fn save(
        &mut self,
        value: Continuation,
    ) -> Result<Arc<ContinuationRecord>, StoreError> {
        validate(&value)?;
        if let Some(existing) = self.records.get(&value.id) {
            return if existing.continuation == value {
                Ok(existing.clone())
            } else {
                Err(StoreError::IdentityConflict)
            };
        }
        let key = (
            value.adapter.clone(),
            value.scope.clone(),
            value.native_key.clone(),
        );
        let previous = self
            .latest
            .get(&key)
            .and_then(|id| self.records.get(id))
            .cloned();
        validate_successor(&value, previous.as_deref())?;
        if let Some(previous) = previous {
            let mut previous = (*previous).clone();
            previous.latest = false;
            self.records
                .insert(previous.continuation.id.clone(), Arc::new(previous));
        }
        self.latest.insert(key, value.id.clone());
        let record = Arc::new(ContinuationRecord {
            continuation: value,
            state: ContinuationState::Available,
            latest: true,
        });
        self.records
            .insert(record.continuation.id.clone(), record.clone());
        Ok(record)
    }

    pub(super) fn get(&self, id: &ContinuationId) -> Result<Arc<ContinuationRecord>, StoreError> {
        self.records
            .get(id)
            .cloned()
            .ok_or(StoreError::MissingContinuation)
    }

    pub(super) fn claim(
        &mut self,
        id: &ContinuationId,
    ) -> Result<Arc<ContinuationRecord>, StoreError> {
        let record = self.get(id)?;
        if record.state != ContinuationState::Available || !record.latest {
            return Err(StoreError::ContinuationClaimed);
        }
        let mut record = (*record).clone();
        record.state = ContinuationState::Claimed;
        let record = Arc::new(record);
        self.records.insert(id.clone(), record.clone());
        Ok(record)
    }
}
