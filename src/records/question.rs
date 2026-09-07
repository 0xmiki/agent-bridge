use super::StoreError;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct Question {
    pub title: String,
    pub fields: Vec<QuestionField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct QuestionField {
    pub id: String,
    pub label: String,
    pub required: bool,
    pub kind: QuestionFieldKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(deny_unknown_fields))]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "sqlite",
    serde(
        tag = "type",
        content = "data",
        rename_all = "snake_case",
        deny_unknown_fields
    )
)]
pub enum QuestionFieldKind {
    Text { max_bytes: usize },
    Boolean,
    Integer { min: i64, max: i64 },
    Select { options: Vec<QuestionOption> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "sqlite",
    serde(
        tag = "type",
        content = "data",
        rename_all = "snake_case",
        deny_unknown_fields
    )
)]
pub enum AnswerValue {
    Text(String),
    Boolean(bool),
    Integer(i64),
    Selected(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "sqlite",
    serde(
        tag = "type",
        content = "data",
        rename_all = "snake_case",
        deny_unknown_fields
    )
)]
pub enum AnswerOutcome {
    Submitted(BTreeMap<String, AnswerValue>),
    Declined,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "sqlite", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "sqlite", serde(rename_all = "snake_case"))]
pub enum AnswerDelivery {
    /// Saved locally; no provider delivery is implied.
    Stored,
    Queued,
    Unknown,
}

impl Question {
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.title.trim().is_empty() || self.fields.is_empty() || self.fields.len() > 64 {
            return Err(StoreError::InvalidPayload);
        }
        let mut fields = HashSet::new();
        for field in &self.fields {
            if field.id.trim().is_empty()
                || field.label.trim().is_empty()
                || !fields.insert(&field.id)
            {
                return Err(StoreError::InvalidPayload);
            }
            match &field.kind {
                QuestionFieldKind::Text { max_bytes: 0 } => return Err(StoreError::InvalidPayload),
                QuestionFieldKind::Integer { min, max } if min > max => {
                    return Err(StoreError::InvalidPayload);
                }
                QuestionFieldKind::Select { options } => {
                    let mut ids = HashSet::new();
                    if options.is_empty()
                        || options.len() > 256
                        || options.iter().any(|option| {
                            option.id.trim().is_empty()
                                || option.label.trim().is_empty()
                                || !ids.insert(&option.id)
                        })
                    {
                        return Err(StoreError::InvalidPayload);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn validate_answer(&self, outcome: &AnswerOutcome) -> Result<(), StoreError> {
        self.validate()?;
        let AnswerOutcome::Submitted(values) = outcome else {
            return Ok(());
        };
        if values
            .keys()
            .any(|key| !self.fields.iter().any(|field| &field.id == key))
        {
            return Err(StoreError::InvalidDecision);
        }
        for field in &self.fields {
            let Some(value) = values.get(&field.id) else {
                if field.required {
                    return Err(StoreError::InvalidDecision);
                }
                continue;
            };
            let valid = match (&field.kind, value) {
                (QuestionFieldKind::Text { max_bytes }, AnswerValue::Text(text)) => {
                    text.len() <= *max_bytes && (!field.required || !text.trim().is_empty())
                }
                (QuestionFieldKind::Boolean, AnswerValue::Boolean(_)) => true,
                (QuestionFieldKind::Integer { min, max }, AnswerValue::Integer(value)) => {
                    (*min..=*max).contains(value)
                }
                (QuestionFieldKind::Select { options }, AnswerValue::Selected(id)) => {
                    options.iter().any(|option| &option.id == id)
                }
                _ => false,
            };
            if !valid {
                return Err(StoreError::InvalidDecision);
            }
        }
        Ok(())
    }
}
