//! Configuration intent and provider reports, independent of any transport.
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "acp", feature = "sqlite"),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(
    any(feature = "acp", feature = "sqlite"),
    serde(
        tag = "type",
        content = "value",
        rename_all = "snake_case",
        deny_unknown_fields
    )
)]
pub enum ConfigValue {
    Select(String),
    Boolean(bool),
}

pub type ConfigValues = BTreeMap<String, ConfigValue>;

/// Frozen at dispatch. Reports concern provider configuration, not an attestation
/// of which remote model actually served every generation or delegated task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "acp", feature = "sqlite"),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(any(feature = "acp", feature = "sqlite"), serde(deny_unknown_fields))]
pub struct RunConfiguration {
    /// Explicit selections successfully acknowledged on this session handle.
    pub requested: ConfigValues,
    /// None means no reliable report. Only present keys have reported values.
    pub confirmed: Option<ConfigValues>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigChoice {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    /// A UI hint, not permission or execution authority.
    pub category: Option<String>,
    pub current: ConfigValue,
    /// Choices for Select; empty for Boolean. Provider ordering is retained.
    pub choices: Vec<ConfigChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfiguration {
    /// None means the provider did not expose configuration options.
    pub options: Option<Vec<ConfigOption>>,
    pub values: RunConfiguration,
    pub pending: bool,
    /// A change or report has an uncertain outcome; dispatch is blocked.
    pub uncertain: bool,
}
