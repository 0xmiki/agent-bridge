use std::{error::Error, fmt};

/// An identifier must contain at least one non-whitespace character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidId;

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an identifier cannot be empty or whitespace-only")
    }
}

impl Error for InvalidId {}

macro_rules! identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        ///
        /// Values are caller-assigned and preserved verbatim. Uniqueness belongs
        /// to the runtime or storage layer. This value is not a filesystem path.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidId> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(InvalidId);
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

identifier!(SlotId, "Identity of configured compute capacity.");
identifier!(SessionId, "Identity of an application context boundary.");
identifier!(RunId, "Identity of one execution assignment.");
identifier!(
    RecordId,
    "Identity of an input, output, or activity record."
);
identifier!(ResourceId, "Identity of referenced content.");
identifier!(
    ActorId,
    "Application-owned attribution, independent of an executor."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_blank_identifiers_without_rewriting_valid_ones() {
        for value in ["", " ", "\n\t", "\u{2003}"] {
            assert_eq!(SessionId::new(value), Err(InvalidId));
        }
        let id = SessionId::new("external/session:123").unwrap();
        assert_eq!(id.as_str(), "external/session:123");
    }
}
