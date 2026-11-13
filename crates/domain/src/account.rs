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
    calendar: CalendarCapabilities,
    mail_write_permission_review: bool,
}

impl AccountCapabilities {
    /// Creates capabilities for the supported mail and calendar actions.
    #[must_use]
    pub const fn new(mail: MailCapabilities, calendar: CalendarCapabilities) -> Self {
        Self {
            mail,
            calendar,
            mail_write_permission_review: false,
        }
    }

    /// Returns read-only mail capabilities with no calendar access.
    #[must_use]
    pub fn read_only_mail() -> Self {
        Self::new(
            MailCapabilities::read_only(),
            CalendarCapabilities::unavailable(),
        )
    }

    /// Returns all current mail and calendar actions.
    #[must_use]
    pub fn full_access() -> Self {
        Self::new(
            MailCapabilities::full_access(),
            CalendarCapabilities::full_access(),
        )
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

    /// Returns the account's supported calendar actions.
    #[must_use]
    pub const fn calendar(self) -> CalendarCapabilities {
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

/// A calendar operation supported by an account's provider.
#[non_exhaustive]
#[derive(Debug, Hash, Ord, PartialOrd, EnumSetType)]
pub enum CalendarAction {
    /// View calendar events.
    ViewEvents,
    /// Create calendar events.
    CreateEvent,
    /// Update calendar events.
    UpdateEvent,
    /// Delete calendar events.
    DeleteEvent,
}

const CALENDAR_WRITE_ACTIONS: [CalendarAction; 3] = [
    CalendarAction::CreateEvent,
    CalendarAction::UpdateEvent,
    CalendarAction::DeleteEvent,
];

/// The calendar operations supported by an account's provider.
///
/// An empty set means calendar data is unavailable. Any write action also
/// includes [`CalendarAction::ViewEvents`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CalendarCapabilities(EnumSet<CalendarAction>);

impl CalendarCapabilities {
    /// Returns a capability set with no calendar actions.
    #[must_use]
    pub fn unavailable() -> Self {
        Self::default()
    }

    /// Returns a capability set that can view events without changing them.
    #[must_use]
    pub fn view_only() -> Self {
        Self::from_actions([CalendarAction::ViewEvents])
    }

    /// Creates a capability set from the actions supported by a provider.
    ///
    /// If the set includes a write action, this constructor also adds
    /// [`CalendarAction::ViewEvents`].
    #[must_use]
    pub fn from_actions(actions: impl IntoIterator<Item = CalendarAction>) -> Self {
        let mut actions: EnumSet<CalendarAction> = actions.into_iter().collect();
        if CALENDAR_WRITE_ACTIONS
            .into_iter()
            .any(|action| actions.contains(action))
        {
            actions.insert(CalendarAction::ViewEvents);
        }
        Self(actions)
    }

    /// Returns all currently supported calendar actions.
    #[must_use]
    pub fn full_access() -> Self {
        Self::from_actions([
            CalendarAction::ViewEvents,
            CalendarAction::CreateEvent,
            CalendarAction::UpdateEvent,
            CalendarAction::DeleteEvent,
        ])
    }

    /// Returns whether the provider supports the specified action.
    #[must_use]
    pub fn supports(self, action: CalendarAction) -> bool {
        self.0.contains(action)
    }

    /// Returns whether the provider supports viewing events without any writes.
    #[must_use]
    pub fn is_read_only(self) -> bool {
        self.supports(CalendarAction::ViewEvents)
            && !CALENDAR_WRITE_ACTIONS
                .into_iter()
                .any(|action| self.supports(action))
    }
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
    /// Creates a profile with provider-selected capabilities and at least one
    /// user-facing identity value.
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
mod tests;
