//! Opaque identifiers for provider-backed domain entities.

use std::fmt;

/// The supplied provider identifier was empty.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyIdentifier;

impl fmt::Display for EmptyIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("identifier must not be empty")
    }
}

impl std::error::Error for EmptyIdentifier {}

fn validate_identifier(provider_identifier: String) -> Result<Box<str>, EmptyIdentifier> {
    if provider_identifier.is_empty() {
        return Err(EmptyIdentifier);
    }

    Ok(provider_identifier.into_boxed_str())
}

macro_rules! define_identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        pub struct $name(Box<str>);

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
define_identifier!(CalendarId, "An opaque identifier for a calendar.");
define_identifier!(EventId, "An opaque identifier for a calendar event.");

#[cfg(test)]
mod tests {
    use super::{EmptyIdentifier, MessageId};

    #[test]
    fn rejects_empty_provider_identifiers() {
        assert_eq!(MessageId::try_from(String::new()), Err(EmptyIdentifier));
    }

    #[test]
    fn preserves_non_empty_provider_identifiers_exactly() {
        let message_id = MessageId::try_from("AbC/9-Q".to_owned())
            .expect("a non-empty synthetic provider ID is valid");

        assert_eq!(message_id.as_str(), "AbC/9-Q");
    }
}
