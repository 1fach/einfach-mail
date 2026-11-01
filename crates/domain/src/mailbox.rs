use std::fmt;

/// The mailbox address contains no non-whitespace characters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct EmptyMailboxAddress;

impl fmt::Display for EmptyMailboxAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mailbox address must not be empty")
    }
}

impl std::error::Error for EmptyMailboxAddress {}

/// A mailbox identity associated with an account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mailbox {
    display_name: Option<String>,
    address: String,
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
}
