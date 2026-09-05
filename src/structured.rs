//! Host validation of a JSON result, independent of provider enforcement.
use serde::{Serialize, de::DeserializeOwned};
use std::{error::Error, fmt, marker::PhantomData};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum JsonRejection {
    MissingOutput,
    AmbiguousOutput,
    NonTextOutput,
    OutputTooLarge,
    InvalidJson(String),
    InvalidShape(String),
    InvalidValue(String),
    Incomplete(String),
}
impl fmt::Display for JsonRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON result rejected: {self:?}")
    }
}
impl Error for JsonRejection {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidContract;
impl fmt::Display for InvalidContract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "contract name, revision, instructions, and validation byte limit must be nonempty",
        )
    }
}
impl Error for InvalidContract {}

type ValueCheck<T> = dyn Fn(&T) -> Result<(), String> + Send + Sync;

/// The name/revision identify host-owned validation code. Instructions describe
/// the requested output to the provider; they do not enforce the Rust type.
pub struct JsonContract<T> {
    name: String,
    revision: String,
    instructions: String,
    max_validation_bytes: usize,
    check: Option<Box<ValueCheck<T>>>,
    output: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> JsonContract<T> {
    pub fn new(
        name: impl Into<String>,
        revision: impl Into<String>,
        instructions: impl Into<String>,
        max_validation_bytes: usize,
    ) -> Result<Self, InvalidContract> {
        let name = name.into();
        let revision = revision.into();
        let instructions = instructions.into();
        if name.trim().is_empty()
            || revision.trim().is_empty()
            || instructions.trim().is_empty()
            || max_validation_bytes == 0
        {
            return Err(InvalidContract);
        }
        Ok(Self {
            name,
            revision,
            instructions,
            max_validation_bytes,
            check: None,
            output: PhantomData,
        })
    }
    #[must_use]
    pub fn with_validation(
        mut self,
        check: impl Fn(&T) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.check = Some(Box::new(check));
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub fn instructions(&self) -> &str {
        &self.instructions
    }
    pub fn max_validation_bytes(&self) -> usize {
        self.max_validation_bytes
    }
    pub fn has_application_validation(&self) -> bool {
        self.check.is_some()
    }

    /// Parse exactly one JSON value without fences, extraction, coercion, or repair.
    /// Field/shape acceptance is defined by T's serde Deserialize implementation.
    pub fn validate(&self, text: &str) -> Result<T, JsonRejection> {
        if text.len() > self.max_validation_bytes {
            return Err(JsonRejection::OutputTooLarge);
        }
        if text.trim().is_empty() {
            return Err(JsonRejection::MissingOutput);
        }
        let value: T = serde_json::from_str(text).map_err(|error| match error.classify() {
            serde_json::error::Category::Data => JsonRejection::InvalidShape(error.to_string()),
            _ => JsonRejection::InvalidJson(error.to_string()),
        })?;
        if let Some(check) = &self.check {
            check(&value).map_err(JsonRejection::InvalidValue)?;
        }
        Ok(value)
    }
}
