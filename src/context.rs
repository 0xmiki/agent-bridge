//! Resolve explicit inputs without dispatching work or choosing provider semantics.
use crate::records::{Payload, RecordStore, Snapshot, StoreError};
use crate::{Content, ContextManifest, InstructionRef, ResourceId, ResourceRef, SessionId};
use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

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
    pub manifest: ContextManifest,
    pub records: Vec<Arc<Snapshot>>,
    pub instructions: Vec<PreparedInstruction>,
    /// Unique resources in first-reference order, including message attachments.
    pub resources: Vec<Arc<Resource>>,
    pub resource_bytes: usize,
}

#[derive(Debug)]
pub enum ContextError {
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
    manifest
        .records
        .len()
        .checked_add(manifest.instructions.len())
        .and_then(|count| count.checked_add(manifest.resources.len()))
        .filter(|count| *count <= limits.max_items)
        .ok_or(ContextError::ItemLimit)?;
    let mut remaining = limits.max_items;
    let mut consume = || {
        remaining = remaining.checked_sub(1).ok_or(ContextError::ItemLimit)?;
        Ok::<_, ContextError>(())
    };
    let mut prepared = PreparedContext {
        manifest: manifest.clone(),
        records: vec![],
        instructions: vec![],
        resources: vec![],
        resource_bytes: 0,
    };
    let mut resolved: HashMap<ResourceKey, Arc<Resource>> = HashMap::new();
    let mut resolve = |reference: &ResourceRef| -> Result<Arc<Resource>, ContextError> {
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
