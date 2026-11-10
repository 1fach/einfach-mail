use std::fmt;

use thiserror::Error;

/// The mailbox address contains no non-whitespace characters.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("mailbox address must not be empty")]
#[must_use]
pub struct EmptyMailboxAddress;

/// A mailbox identity associated with an account.
#[derive(Clone, Eq, PartialEq)]
pub struct Mailbox {
    display_name: Option<String>,
    address: String,
}

impl fmt::Debug for Mailbox {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Mailbox")
            .field("display_name", &"<redacted>")
            .field("address", &"<redacted>")
            .finish()
    }
}

impl Mailbox {
    /// Creates a mailbox identity from a non-empty address and optional display name.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyMailboxAddress`] if the address is empty or contains only whitespace.
    pub fn try_new(
        address: String,
        display_name: Option<String>,
    ) -> Result<Self, EmptyMailboxAddress> {
        if address.trim().is_empty() {
            return Err(EmptyMailboxAddress);
        }

        Ok(Self {
            display_name,
            address,
        })
    }

    /// Returns the display name, when one is available.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Returns the mailbox address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }
}

#[cfg(test)]
mod tests {
    use super::{EmptyMailboxAddress, Mailbox};

    #[test]
    fn mailbox_requires_a_non_empty_address() {
        for address in [String::new(), " \t\n".to_owned()] {
            assert_eq!(Mailbox::try_new(address, None), Err(EmptyMailboxAddress));
        }
    }

    #[test]
    fn mailbox_debug_output_redacts_address_and_display_name() {
        let mailbox = Mailbox::try_new(
            "mailbox-address-marker@example.test".to_owned(),
            Some("mailbox-display-marker".to_owned()),
        )
        .expect("synthetic mailbox is valid");

        let debug = format!("{mailbox:?}");

        assert!(
            !debug.contains("mailbox-address-marker"),
            "Debug output included the mailbox address: {debug}"
        );
        assert!(
            !debug.contains("mailbox-display-marker"),
            "Debug output included the mailbox display name: {debug}"
        );
    }
}
