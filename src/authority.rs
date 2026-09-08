use crate::{ActorId, SessionId, SlotId};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "sqlite", feature = "receipts"),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(
    any(feature = "sqlite", feature = "receipts"),
    serde(deny_unknown_fields)
)]
pub struct ToolRef {
    pub name: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "sqlite", feature = "receipts"),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(
    any(feature = "sqlite", feature = "receipts"),
    serde(deny_unknown_fields)
)]
pub struct ToolScope {
    pub session: SessionId,
    pub slot: SlotId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "sqlite", feature = "receipts"),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(
    any(feature = "sqlite", feature = "receipts"),
    serde(deny_unknown_fields)
)]
pub struct ToolGrant {
    pub issuer: ActorId,
    pub subject: ActorId,
    pub scope: ToolScope,
    pub tools: Vec<ToolRef>,
}
