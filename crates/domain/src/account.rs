//! Provider-independent identity for a connected account.

use std::fmt;

use crate::{AccountId, Mailbox};

/// The profile has no non-empty field that can identify the account to a user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MissingAccountLabel;

impl fmt::Display for MissingAccountLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("account profile has no display name or address")
    }
}

impl std::error::Error for MissingAccountLabel {}

/// Provider-independent identity details for one connected account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountProfile {
    id: AccountId,
    display_name: Option<String>,
    mailbox: Option<Mailbox>,
    user_principal_name: Option<String>,
}

impl AccountProfile {
    /// Creates an account profile with at least one user-facing identity value.
    ///
    /// Empty or whitespace-only optional values are discarded. The mailbox
    /// address remains separate from the sign-in user principal name.
    ///
    /// # Errors
    ///
    /// Returns [`MissingAccountLabel`] when all display values are empty.
    pub fn try_new(
        id: AccountId,
        display_name: Option<String>,
        mailbox_address: Option<String>,
        user_principal_name: Option<String>,
    ) -> Result<Self, MissingAccountLabel> {
        let display_name = non_empty(display_name);
        let mailbox_address = non_empty(mailbox_address);
        let user_principal_name = non_empty(user_principal_name);
        if display_name.is_none() && mailbox_address.is_none() && user_principal_name.is_none() {
            return Err(MissingAccountLabel);
        }

        let mailbox = mailbox_address
            .map(|address| Mailbox::try_new(address, None))
            .transpose()
            .map_err(|_| MissingAccountLabel)?;

        Ok(Self {
            id,
            display_name,
            mailbox,
            user_principal_name,
        })
    }

    /// Returns the provider-issued account ID.
    #[must_use]
    pub fn id(&self) -> &AccountId {
        &self.id
    }

    /// Returns the account's display name when the provider supplied one.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Returns the mailbox address when the provider supplied one.
    #[must_use]
    pub fn mailbox(&self) -> Option<&Mailbox> {
        self.mailbox.as_ref()
    }

    /// Returns the sign-in user principal name when the provider supplied one.
    #[must_use]
    pub fn user_principal_name(&self) -> Option<&str> {
        self.user_principal_name.as_deref()
    }

    /// Returns the best available label for the connected account.
    ///
    /// # Panics
    ///
    /// Panics if the profile has no display name, mailbox address, or user principal name.
    /// [`Self::try_new`] rejects profiles with none of those values.
    #[must_use]
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .or_else(|| self.mailbox.as_ref().map(Mailbox::address))
            .or(self.user_principal_name.as_deref())
            .expect("AccountProfile requires a non-empty display value")
    }
}

fn non_empty(candidate: Option<String>) -> Option<String> {
    candidate.and_then(|candidate| {
        let trimmed = candidate.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::{AccountProfile, MissingAccountLabel};
    use crate::AccountId;

    fn account_id() -> AccountId {
        AccountId::try_from("account-id".to_owned()).expect("synthetic ID is non-empty")
    }

    #[test]
    fn prefers_display_name_for_the_account_label() {
        let profile = AccountProfile::try_new(
            account_id(),
            Some("  Ada Example  ".to_owned()),
            Some("ada@example.test".to_owned()),
            Some("ada@tenant.example.test".to_owned()),
        )
        .expect("profile includes display values");

        assert_eq!(profile.label(), "Ada Example");
        assert_eq!(
            profile.mailbox().map(|mailbox| mailbox.address()),
            Some("ada@example.test")
        );
        assert_eq!(
            profile.user_principal_name(),
            Some("ada@tenant.example.test")
        );
    }

    #[test]
    fn falls_back_to_mailbox_then_user_principal_name() {
        let mailbox_profile = AccountProfile::try_new(
            account_id(),
            None,
            Some("ada@example.test".to_owned()),
            Some("ada@tenant.example.test".to_owned()),
        )
        .expect("mailbox identifies the account");
        let upn_profile = AccountProfile::try_new(
            account_id(),
            None,
            None,
            Some("ada@tenant.example.test".to_owned()),
        )
        .expect("user principal name identifies the account");

        assert_eq!(mailbox_profile.label(), "ada@example.test");
        assert_eq!(upn_profile.label(), "ada@tenant.example.test");
    }

    #[test]
    fn rejects_profiles_without_a_user_facing_identity() {
        let profile_result =
            AccountProfile::try_new(account_id(), Some("  ".to_owned()), None, None);

        assert_eq!(profile_result, Err(MissingAccountLabel));
    }
}
