//! Host-authorized bridge-managed child execution. Not a scheduler or native subagent API.
use crate::records::{RecordStore, StoreError};
use crate::{ActorId, ContextManifest, RunId, RunSpec, SlotId, ToolGrant, ToolRef, ToolScope};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct DelegationGrant {
    pub issuer: ActorId,
    pub subject: ActorId,
    pub parent: RunId,
    pub context: ContextManifest,
    pub tools: Vec<ToolRef>,
}

#[derive(Debug, Clone)]
pub struct ChildRequest {
    pub id: RunId,
    pub slot: SlotId,
    pub actor: ActorId,
    pub context: ContextManifest,
    pub tools: Vec<ToolRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct ExecutionRelation {
    pub parent: RunId,
    pub child: RunId,
    pub parent_authority: ToolGrant,
    pub delegation: DelegationGrant,
    pub child_authority: ToolGrant,
}

impl ExecutionRelation {
    pub fn validate(&self, parent: &RunSpec, child: &RunSpec) -> Result<(), StoreError> {
        let parent_scope = ToolScope {
            session: parent.session_id.clone(),
            slot: parent.slot_id.clone(),
        };
        let child_scope = ToolScope {
            session: child.session_id.clone(),
            slot: child.slot_id.clone(),
        };
        if self.parent == self.child
            || self.parent != parent.id
            || self.child != child.id
            || self.delegation.parent != parent.id
            || parent.session_id != child.session_id
            || child.continuation.is_some()
            || self.parent_authority.scope != parent_scope
            || self.child_authority.scope != child_scope
            || self.parent_authority.issuer != self.delegation.issuer
            || self.child_authority.issuer != self.delegation.issuer
            || self.parent_authority.subject != self.delegation.subject
            || self.child_authority.tools.iter().any(|tool| {
                !self.parent_authority.tools.contains(tool) || !self.delegation.tools.contains(tool)
            })
            || child
                .context
                .records
                .iter()
                .any(|id| !self.delegation.context.records.contains(id))
            || child
                .context
                .resources
                .iter()
                .any(|reference| !self.delegation.context.resources.contains(reference))
            || child
                .context
                .instructions
                .iter()
                .any(|reference| !self.delegation.context.instructions.contains(reference))
        {
            return Err(StoreError::InvalidExecutionRelation);
        }
        Ok(())
    }
}

pub trait ExecutionStore: RecordStore {
    /// Both runs must exist. One immutable parent edge per child; cycles fail.
    fn link_execution(
        &self,
        relation: ExecutionRelation,
    ) -> Result<Arc<ExecutionRelation>, StoreError>;
    fn execution_parent(&self, child: &RunId)
    -> Result<Option<Arc<ExecutionRelation>>, StoreError>;
    fn execution_children(&self, parent: &RunId)
    -> Result<Vec<Arc<ExecutionRelation>>, StoreError>;
}

/// A checked selection. It does not dispatch, grant native tools, or inherit history.
#[derive(Debug, Clone)]
pub struct ChildPlan {
    parent: RunSpec,
    request: ChildRequest,
    relation: ExecutionRelation,
}
impl ChildPlan {
    pub fn new(
        store: &(impl ExecutionStore + ?Sized),
        parent_authority: ToolGrant,
        delegation: DelegationGrant,
        request: ChildRequest,
    ) -> Result<Self, StoreError> {
        let parent = store.get_run(&delegation.parent)?;
        if store
            .execution_parent(&parent.id)?
            .is_some_and(|edge| edge.child_authority != parent_authority)
        {
            return Err(StoreError::InvalidExecutionRelation);
        }
        let child_authority = ToolGrant {
            issuer: delegation.issuer.clone(),
            subject: request.actor.clone(),
            scope: ToolScope {
                session: parent.session_id.clone(),
                slot: request.slot.clone(),
            },
            tools: request.tools.clone(),
        };
        let relation = ExecutionRelation {
            parent: parent.id.clone(),
            child: request.id.clone(),
            parent_authority,
            delegation,
            child_authority,
        };
        let provisional = RunSpec {
            id: request.id.clone(),
            session_id: parent.session_id.clone(),
            slot_id: request.slot.clone(),
            context: request.context.clone(),
            config: Default::default(),
            continuation: None,
        };
        relation.validate(&parent, &provisional)?;
        Ok(Self {
            parent,
            request,
            relation,
        })
    }
    pub fn request(&self) -> &ChildRequest {
        &self.request
    }
    pub fn authority(&self) -> &ToolGrant {
        &self.relation.child_authority
    }
    pub fn validate_run(
        &self,
        store: &(impl ExecutionStore + ?Sized),
        child: &RunSpec,
    ) -> Result<ExecutionRelation, StoreError> {
        if store.get_run(&self.parent.id)? != self.parent || child.context != self.request.context {
            return Err(StoreError::InvalidExecutionRelation);
        }
        if store
            .execution_parent(&self.parent.id)?
            .is_some_and(|edge| edge.child_authority != self.relation.parent_authority)
        {
            return Err(StoreError::InvalidExecutionRelation);
        }
        self.relation.validate(&self.parent, child)?;
        Ok(self.relation.clone())
    }
}
