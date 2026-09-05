use super::*;

pub(super) fn same_draft(snapshot: &Snapshot, draft: &Draft) -> bool {
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

pub(super) fn validate(payload: &Payload, state: RecordState) -> Result<(), StoreError> {
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

pub(super) fn checkpoint(
    current: &Arc<Snapshot>,
    expected_revision: u64,
    payload: Payload,
    next: RecordState,
) -> Result<Arc<Snapshot>, StoreError> {
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
    let mut snapshot = (**current).clone();
    snapshot.record.payload = payload;
    snapshot.state = next;
    snapshot.revision = snapshot
        .revision
        .checked_add(1)
        .ok_or(StoreError::SequenceExhausted)?;
    Ok(Arc::new(snapshot))
}

pub(super) fn resolve_request(
    original: &Snapshot,
    expected_revision: u64,
    decision: &Draft,
) -> Result<Snapshot, StoreError> {
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
    if decision.reply_to_id.as_ref() != Some(&original.record.id)
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
    let mut resolved = original.clone();
    resolved.state = RecordState::Complete;
    resolved.revision = resolved
        .revision
        .checked_add(1)
        .ok_or(StoreError::SequenceExhausted)?;
    Ok(resolved)
}
