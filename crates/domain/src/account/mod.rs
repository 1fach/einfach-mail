//! Provider-independent identity for a connected account.

use std::fmt;

use enumset::{EnumSet, EnumSetType};
use thiserror::Error;

use crate::{AccountId, Mailbox};

/// The profile has no non-empty field that can identify the account to a user.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("account profile has no display name or address")]
pub struct MissingAccountLabel;

/// Mail and calendar actions supported by a connected account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountCapabilities {
    mail: MailCapabilities,
    calendar: CalendarAccess,
    mail_write_permission_review: bool,
}

impl AccountCapabilities {
    /// Creates capabilities for the supported mail and calendar actions.
    #[must_use]
    pub const fn new(mail: MailCapabilities, calendar: CalendarAccess) -> Self {
        Self {
            mail,
            calendar,
            mail_write_permission_review: false,
        }
    }

    /// Returns read-only mail capabilities with no calendar access.
    #[must_use]
    pub fn read_only_mail() -> Self {
        Self::new(MailCapabilities::read_only(), CalendarAccess::Unavailable)
    }

    /// Returns all current mail actions and read/write calendar access.
    #[must_use]
    pub fn full_access() -> Self {
        Self::new(MailCapabilities::full_access(), CalendarAccess::ReadWrite)
            .with_mail_write_permission_review()
    }

    /// Enables the Microsoft Graph mail permission review flow for this account.
    #[must_use]
    pub const fn with_mail_write_permission_review(mut self) -> Self {
        self.mail_write_permission_review = true;
        self
    }

    /// Returns whether this account can request Microsoft Graph mail write permission.
    #[must_use]
    pub const fn can_review_mail_write_permission(self) -> bool {
        self.mail_write_permission_review
    }

    /// Returns the account's supported mail actions.
    #[must_use]
    pub const fn mail(self) -> MailCapabilities {
        self.mail
    }

    /// Returns the account's calendar access level.
    #[must_use]
    pub const fn calendar(self) -> CalendarAccess {
        self.calendar
    }
}

/// A mail operation supported by an account's provider.
#[non_exhaustive]
#[derive(Debug, Hash, Ord, PartialOrd, EnumSetType)]
pub enum MailAction {
    /// Set a message's read or unread state.
    SetReadState,
    /// Set or clear a message's follow-up flag.
    SetFollowUpFlagStatus,
    /// Send a new message.
    Send,
    /// Reply to a message.
    Reply,
    /// Move or archive a message.
    Move,
    /// Move a message to Deleted Items.
    Delete,
}

/// The mail operations supported by an account's provider.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MailCapabilities(EnumSet<MailAction>);

impl MailCapabilities {
    /// Returns a capability set with no mail write actions.
    #[must_use]
    pub fn read_only() -> Self {
        Self(EnumSet::empty())
    }

    /// Creates a capability set from the actions supported by a provider.
    #[must_use]
    pub fn from_actions(actions: impl IntoIterator<Item = MailAction>) -> Self {
        Self(actions.into_iter().collect())
    }

    /// Returns the currently supported mail actions.
    #[must_use]
    pub fn full_access() -> Self {
        Self::from_actions([
            MailAction::SetReadState,
            MailAction::SetFollowUpFlagStatus,
            MailAction::Send,
            MailAction::Reply,
            MailAction::Move,
            MailAction::Delete,
        ])
    }

    /// Returns whether the provider supports the specified action.
    #[must_use]
    pub fn supports(self, action: MailAction) -> bool {
        self.0.contains(action)
    }

    /// Returns whether the account has no mail write actions.
    #[must_use]
    pub fn is_read_only(self) -> bool {
        self.0.is_empty()
    }
}

/// The calendar access available for an account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarAccess {
    /// The account does not expose calendar data.
    Unavailable,
    /// The account can read calendar data.
    ReadOnly,
    /// The account can read and change calendar data.
    ReadWrite,
}

/// Provider-independent identity details for one connected account.
#[derive(Clone, Eq, PartialEq)]
pub struct AccountProfile {
    id: AccountId,
    display_name: Option<String>,
    mailbox: Option<Mailbox>,
    user_principal_name: Option<String>,
    capabilities: AccountCapabilities,
}

impl fmt::Debug for AccountProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountProfile")
            .field("id", &self.id)
            .field("display_name", &"<redacted>")
            .field("mailbox", &self.mailbox)
            .field("user_principal_name", &"<redacted>")
            .field("capabilities", &self.capabilities)
            .finish()
    }
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
        Self::try_new_with_capabilities(
            id,
            display_name,
            mailbox_address,
            user_principal_name,
            AccountCapabilities::full_access(),
        )
    }

    /// Creates an account profile with explicit mail and calendar capabilities.
    ///
    /// Empty or whitespace-only optional values are discarded. The mailbox
    /// address remains separate from the sign-in user principal name.
    ///
    /// # Errors
    ///
    /// Returns [`MissingAccountLabel`] when all display values are empty.
    pub fn try_new_with_capabilities(
        id: AccountId,
        display_name: Option<String>,
        mailbox_address: Option<String>,
        user_principal_name: Option<String>,
        capabilities: AccountCapabilities,
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
            capabilities,
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

    /// Returns the actions supported by this account's provider.
    #[must_use]
    pub fn capabilities(&self) -> AccountCapabilities {
        self.capabilities
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
    use super::{
        AccountCapabilities, AccountProfile, CalendarAccess, MailAction, MailCapabilities,
        MissingAccountLabel,
    };
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

    #[test]
    fn account_profile_debug_output_redacts_identity_values() {
        let id = AccountId::try_from("account-id-marker".to_owned())
            .expect("synthetic account ID is non-empty");
        let profile = AccountProfile::try_new(
            id,
            Some("account-display-marker".to_owned()),
            Some("mailbox-address-marker@example.test".to_owned()),
            Some("user-principal-marker@example.test".to_owned()),
        )
        .expect("synthetic account profile is valid");

        let debug = format!("{profile:?}");

        for marker in [
            "account-id-marker",
            "account-display-marker",
            "mailbox-address-marker",
            "user-principal-marker",
        ] {
            assert!(!debug.contains(marker), "Debug output included {marker:?}");
        }
    }

    #[test]
    fn profile_retains_read_only_mail_capabilities() {
        let profile = AccountProfile::try_new_with_capabilities(
            account_id(),
            None,
            Some("ada@example.test".to_owned()),
            None,
            AccountCapabilities::read_only_mail(),
        )
        .expect("mailbox identifies the account");

        assert!(profile.capabilities().mail().is_read_only());
        assert!(!profile.capabilities().can_review_mail_write_permission());
        assert_eq!(
            profile.capabilities().calendar(),
            CalendarAccess::Unavailable
        );
    }

    #[test]
    fn mail_capabilities_only_include_the_requested_actions() {
        let capabilities = MailCapabilities::from_actions([MailAction::SetReadState]);

        assert!(!capabilities.is_read_only());
        assert!(capabilities.supports(MailAction::SetReadState));
        assert!(!capabilities.supports(MailAction::SetFollowUpFlagStatus));
        assert!(!capabilities.supports(MailAction::Send));
        assert!(!capabilities.supports(MailAction::Reply));
        assert!(!capabilities.supports(MailAction::Move));
        assert!(!capabilities.supports(MailAction::Delete));

        let account = AccountCapabilities::new(capabilities, CalendarAccess::ReadOnly);
        assert!(!account.can_review_mail_write_permission());
    }

    #[test]
    fn full_access_includes_every_current_mail_action_and_permission_review() {
        let capabilities = AccountCapabilities::full_access();
        let mail = capabilities.mail();

        assert!(mail.supports(MailAction::SetReadState));
        assert!(mail.supports(MailAction::SetFollowUpFlagStatus));
        assert!(mail.supports(MailAction::Send));
        assert!(mail.supports(MailAction::Reply));
        assert!(mail.supports(MailAction::Move));
        assert!(mail.supports(MailAction::Delete));
        assert!(capabilities.can_review_mail_write_permission());
    }
}
