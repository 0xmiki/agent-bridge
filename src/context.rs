//! Resolve explicit inputs without dispatching work or choosing provider semantics.
use crate::records::{Payload, RecordStore, Snapshot, StoreError};
use crate::{Content, ContextManifest, InstructionRef, ResourceId, ResourceRef, SessionId};
use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

/// Host-issued permission to supply these exact instruction revisions and roles.
/// The host authenticates issuer/requester identities; this is not a bearer credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionGrant {
    pub issuer: crate::ActorId,
    pub subject: crate::ActorId,
    pub instructions: Vec<InstructionRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionAuthorization {
    pub requester: crate::ActorId,
    pub grant: InstructionGrant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextItem {
    Record(crate::RecordId),
    Instruction(InstructionRef),
    Resource(ResourceRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextOmission {
    pub item: ContextItem,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct ContextPolicy {
    pub omissions: Vec<ContextOmission>,
    pub instruction_authorization: Option<Arc<InstructionAuthorization>>,
}

impl ContextPolicy {
    /// Explicit host-owned selection. Delegated selections use a grant naming
    /// their actual requester instead of this convenience constructor.
    pub fn for_host(host: crate::ActorId, instructions: Vec<InstructionRef>) -> Self {
        Self {
            omissions: vec![],
            instruction_authorization: Some(Arc::new(InstructionAuthorization {
                requester: host.clone(),
                grant: InstructionGrant {
                    issuer: host.clone(),
                    subject: host,
                    instructions,
                },
            })),
        }
    }
}

/// Immutable bytes at an application-assigned revision. Revision labels are not hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub reference: ResourceRef,
    pub media_type: String,
    pub bytes: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceError {
    Store(StoreError),
    Missing,
    Invalid,
    RevisionConflict,
    Poisoned,
}
impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "resource store: {self:?}")
    }
}
impl Error for ResourceError {}

/// Stores must return the exact immutable revision, never the latest revision.
/// Access control and retention belong to the application's store implementation.
pub trait ResourceStore: Send + Sync {
    fn get(&self, reference: &ResourceRef) -> Result<Arc<Resource>, ResourceError>;
}

/// Optional write contract. Retention and durability are properties of the store.
pub trait ResourceArchive: ResourceStore {
    fn put(&self, resource: Resource) -> Result<Arc<Resource>, ResourceError>;
}

impl ResourceArchive for MemoryResourceStore {
    fn put(&self, resource: Resource) -> Result<Arc<Resource>, ResourceError> {
        MemoryResourceStore::put(self, resource)
    }
}

type ResourceKey = (ResourceId, String);
#[derive(Default)]
pub struct MemoryResourceStore {
    resources: Mutex<HashMap<ResourceKey, Arc<Resource>>>,
}
impl MemoryResourceStore {
    /// Repeated identical writes share the stored allocation. Conflicting writes fail.
    pub fn put(&self, resource: Resource) -> Result<Arc<Resource>, ResourceError> {
        if resource.reference.revision.trim().is_empty() || resource.media_type.trim().is_empty() {
            return Err(ResourceError::Invalid);
        }
        let key = (
            resource.reference.id.clone(),
            resource.reference.revision.clone(),
        );
        let mut resources = self.resources.lock().map_err(|_| ResourceError::Poisoned)?;
        if let Some(existing) = resources.get(&key) {
            return if **existing == resource {
                Ok(existing.clone())
            } else {
                Err(ResourceError::RevisionConflict)
            };
        }
        let resource = Arc::new(resource);
        resources.insert(key, resource.clone());
        Ok(resource)
    }
}
impl ResourceStore for MemoryResourceStore {
    fn get(&self, reference: &ResourceRef) -> Result<Arc<Resource>, ResourceError> {
        if reference.revision.trim().is_empty() {
            return Err(ResourceError::Invalid);
        }
        self.resources
            .lock()
            .map_err(|_| ResourceError::Poisoned)?
            .get(&(reference.id.clone(), reference.revision.clone()))
            .cloned()
            .ok_or(ResourceError::Missing)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ContextLimits {
    /// Explicit selections plus resource references inside selected messages.
    pub max_items: usize,
    /// Sum of unique resource byte lengths. This is not a model token budget.
    pub max_resource_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct PreparedInstruction {
    pub reference: InstructionRef,
    pub resource: Arc<Resource>,
}

/// Resolved evidence only. It is not a provider prompt or proof of delivery.
#[derive(Debug, Clone)]
pub struct PreparedContext {
    pub requested_manifest: ContextManifest,
    pub policy: ContextPolicy,
    pub manifest: ContextManifest,
    pub records: Vec<Arc<Snapshot>>,
    pub instructions: Vec<PreparedInstruction>,
    /// Unique resources in first-reference order, including message attachments.
    pub resources: Vec<Arc<Resource>>,
    pub resource_bytes: usize,
}

#[derive(Debug)]
pub enum ContextError {
    InstructionUnauthorized,
    InvalidOmission,
    ReferencedOmission(ResourceRef),
    Store(StoreError),
    Resource {
        reference: ResourceRef,
        error: ResourceError,
    },
    RecordOutsideScope(crate::RecordId),
    OpenRecord(crate::RecordId),
    InvalidInstruction(ResourceRef),
    ItemLimit,
    ResourceByteLimit,
}
impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "context preparation: {self:?}")
    }
}
impl Error for ContextError {}
impl From<StoreError> for ContextError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// The host supplies the sessions it permits this preparation to read. Resource
/// access is delegated to the supplied ResourceStore. No omitted input is inferred.
pub fn prepare(
    manifest: &ContextManifest,
    records: &impl RecordStore,
    resources: &impl ResourceStore,
    allowed_sessions: &[SessionId],
    limits: ContextLimits,
) -> Result<PreparedContext, ContextError> {
    prepare_inner(manifest, records, resources, allowed_sessions, limits, &[])
}

/// Policy applies to explicit selections. Missing inputs are omitted only when
/// named here, and omitted instructions still require an exact grant.
pub fn prepare_with_policy(
    manifest: &ContextManifest,
    records: &impl RecordStore,
    resources: &impl ResourceStore,
    allowed_sessions: &[SessionId],
    limits: ContextLimits,
    policy: &ContextPolicy,
) -> Result<PreparedContext, ContextError> {
    check_count(manifest, limits)?;
    if policy.omissions.len() > limits.max_items {
        return Err(ContextError::ItemLimit);
    }
    if policy
        .instruction_authorization
        .as_ref()
        .is_some_and(|authorization| authorization.grant.instructions.len() > limits.max_items)
    {
        return Err(ContextError::ItemLimit);
    }
    match &policy.instruction_authorization {
        Some(authorization)
            if authorization.requester != authorization.grant.subject
                || manifest
                    .instructions
                    .iter()
                    .any(|instruction| !authorization.grant.instructions.contains(instruction)) =>
        {
            return Err(ContextError::InstructionUnauthorized);
        }
        None if !manifest.instructions.is_empty() => {
            return Err(ContextError::InstructionUnauthorized);
        }
        _ => {}
    }
    let mut selected = manifest.clone();
    let mut omitted = vec![];
    let mut forbidden = vec![];
    for omission in &policy.omissions {
        if omission.reason.trim().is_empty() || omitted.contains(&omission.item) {
            return Err(ContextError::InvalidOmission);
        }
        let exists = match &omission.item {
            ContextItem::Record(id) => {
                let exists = selected.records.contains(id);
                selected.records.retain(|item| item != id);
                exists
            }
            ContextItem::Instruction(instruction) => {
                let exists = selected.instructions.contains(instruction);
                selected.instructions.retain(|item| item != instruction);
                exists
            }
            ContextItem::Resource(reference) => {
                let exists = selected.resources.contains(reference);
                selected.resources.retain(|item| item != reference);
                forbidden.push(reference.clone());
                exists
            }
        };
        if !exists {
            return Err(ContextError::InvalidOmission);
        }
        omitted.push(omission.item.clone());
    }
    let mut prepared = prepare_inner(
        &selected,
        records,
        resources,
        allowed_sessions,
        limits,
        &forbidden,
    )?;
    prepared.requested_manifest = manifest.clone();
    prepared.policy = policy.clone();
    Ok(prepared)
}

fn check_count(manifest: &ContextManifest, limits: ContextLimits) -> Result<(), ContextError> {
    manifest
        .records
        .len()
        .checked_add(manifest.instructions.len())
        .and_then(|count| count.checked_add(manifest.resources.len()))
        .filter(|count| *count <= limits.max_items)
        .ok_or(ContextError::ItemLimit)?;
    Ok(())
}

fn prepare_inner(
    manifest: &ContextManifest,
    records: &impl RecordStore,
    resources: &impl ResourceStore,
    allowed_sessions: &[SessionId],
    limits: ContextLimits,
    forbidden: &[ResourceRef],
) -> Result<PreparedContext, ContextError> {
    check_count(manifest, limits)?;
    let mut remaining = limits.max_items;
    let mut consume = || {
        remaining = remaining.checked_sub(1).ok_or(ContextError::ItemLimit)?;
        Ok::<_, ContextError>(())
    };
    let mut prepared = PreparedContext {
        requested_manifest: manifest.clone(),
        policy: ContextPolicy::default(),
        manifest: manifest.clone(),
        records: vec![],
        instructions: vec![],
        resources: vec![],
        resource_bytes: 0,
    };
    let mut resolved: HashMap<ResourceKey, Arc<Resource>> = HashMap::new();
    let mut resolve = |reference: &ResourceRef| -> Result<Arc<Resource>, ContextError> {
        if forbidden.contains(reference) {
            return Err(ContextError::ReferencedOmission(reference.clone()));
        }
        let key = (reference.id.clone(), reference.revision.clone());
        if let Some(resource) = resolved.get(&key) {
            return Ok(resource.clone());
        }
        let resource = resources
            .get(reference)
            .map_err(|error| ContextError::Resource {
                reference: reference.clone(),
                error,
            })?;
        if resource.reference != *reference {
            return Err(ContextError::Resource {
                reference: reference.clone(),
                error: ResourceError::RevisionConflict,
            });
        }
        prepared.resource_bytes = prepared
            .resource_bytes
            .checked_add(resource.bytes.len())
            .filter(|size| *size <= limits.max_resource_bytes)
            .ok_or(ContextError::ResourceByteLimit)?;
        prepared.resources.push(resource.clone());
        resolved.insert(key, resource.clone());
        Ok(resource)
    };
    for id in &manifest.records {
        consume()?;
        let snapshot = records.get(id)?;
        if !allowed_sessions.contains(&snapshot.record.session_id) {
            return Err(ContextError::RecordOutsideScope(id.clone()));
        }
        if !snapshot.state.is_final() {
            return Err(ContextError::OpenRecord(id.clone()));
        }
        if let Payload::Message { message, .. } = &snapshot.record.payload {
            for content in &message.content {
                if let Content::Resource(reference) = content {
                    consume()?;
                    resolve(reference)?;
                }
            }
        }
        prepared.records.push(snapshot);
    }
    for instruction in &manifest.instructions {
        consume()?;
        let resource = resolve(&instruction.resource)?;
        if resource
            .media_type
            .split(';')
            .next()
            .is_none_or(|mime| !matches!(mime.trim(), "text/plain" | "text/markdown"))
            || std::str::from_utf8(&resource.bytes).is_err()
        {
            return Err(ContextError::InvalidInstruction(
                instruction.resource.clone(),
            ));
        }
        prepared.instructions.push(PreparedInstruction {
            reference: instruction.clone(),
            resource,
        });
    }
    for reference in &manifest.resources {
        consume()?;
        resolve(reference)?;
    }
    Ok(prepared)
}
