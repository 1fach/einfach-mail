//! Opaque identifiers for provider-backed domain entities.

use std::fmt;

use thiserror::Error;

/// The supplied provider identifier was empty.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("identifier must not be empty")]
pub struct EmptyIdentifier;

fn validate_identifier(provider_identifier: String) -> Result<Box<str>, EmptyIdentifier> {
    if provider_identifier.is_empty() {
        return Err(EmptyIdentifier);
    }

    Ok(provider_identifier.into_boxed_str())
}

macro_rules! define_identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Eq, Hash, PartialEq)]
        pub struct $name(Box<str>);

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&"<redacted>")
                    .finish()
            }
        }

        impl $name {
            /// Returns the exact provider-issued identifier without normalization.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = EmptyIdentifier;

            fn try_from(provider_identifier: String) -> Result<Self, Self::Error> {
                validate_identifier(provider_identifier).map(Self)
            }
        }
    };
}

define_identifier!(
    AccountId,
    "An opaque identifier for a connected provider account."
);
define_identifier!(FolderId, "An opaque identifier for a mail folder.");
define_identifier!(MessageId, "An opaque identifier for a mail message.");
define_identifier!(EventId, "An opaque identifier for a calendar event.");

#[cfg(test)]
mod tests;
