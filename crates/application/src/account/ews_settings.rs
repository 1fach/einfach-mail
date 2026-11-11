//! Application-owned settings for one Exchange Server EWS account.

use std::{fmt, future::Future};

use thiserror::Error;

use crate::AccountOperationError;

/// Non-secret connection settings for an Exchange Server account.
#[derive(Clone, Eq, PartialEq)]
pub struct ExchangeEwsAccountSettings {
    endpoint: String,
    mailbox_address: String,
    username: String,
}

impl ExchangeEwsAccountSettings {
    /// Creates settings that the EWS provider must validate before use.
    #[must_use]
    pub fn new(endpoint: String, mailbox_address: String, username: String) -> Self {
        Self {
            endpoint,
            mailbox_address,
            username,
        }
    }

    /// Returns the configured endpoint for provider-side validation.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Returns the configured mailbox address for provider-side validation.
    #[must_use]
    pub fn mailbox_address(&self) -> &str {
        &self.mailbox_address
    }

    /// Returns the configured NTLM identity for provider-side validation.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }
}

impl fmt::Debug for ExchangeEwsAccountSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExchangeEwsAccountSettings")
            .field("endpoint", &"<redacted>")
            .field("mailbox_address", &"<redacted>")
            .field("username", &"<redacted>")
            .finish()
    }
}

/// Failure to read or update the saved EWS account settings.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("the saved Exchange account settings are unavailable")]
pub struct ExchangeEwsSettingsError;

/// Stores the non-secret EWS connection settings separately from its password.
pub trait ExchangeEwsAccountSettingsStore: Send + Sync {
    /// Loads the saved settings, or returns `None` when no EWS account is saved.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeEwsSettingsError`] when the settings cannot be read or parsed.
    fn load(
        &self,
    ) -> impl Future<Output = Result<Option<ExchangeEwsAccountSettings>, ExchangeEwsSettingsError>>
    + Send
    + '_;

    /// Saves validated settings without storing a password.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeEwsSettingsError`] when the settings cannot be saved.
    fn save(
        &self,
        settings: ExchangeEwsAccountSettings,
    ) -> impl Future<Output = Result<(), ExchangeEwsSettingsError>> + Send + '_;

    /// Removes saved settings; removing an absent record succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeEwsSettingsError`] when the settings cannot be removed.
    fn delete(&self) -> impl Future<Output = Result<(), ExchangeEwsSettingsError>> + Send + '_;
}

impl From<ExchangeEwsSettingsError> for AccountOperationError {
    fn from(_: ExchangeEwsSettingsError) -> Self {
        Self::AccountStateUnavailable
    }
}
