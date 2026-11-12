use std::future::Future;

use einfach_mail_domain::AccountId;
use thiserror::Error;

use crate::MailCacheError;
use crate::shared::reservations::OperationReservations;

/// Errors returned while opening a cache shared by account features.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AccountCacheOpenError {
    /// The cache, credentials, or shared account state are unavailable.
    #[error("the account cache is unavailable")]
    Unavailable,
    /// Another cache handle or lifecycle operation owns an account lock.
    #[error("the account cache is in use")]
    InUse,
    /// The account cache exists but its encryption key is missing or malformed.
    #[error("the account cache encryption key is unavailable")]
    MissingEncryptionKey,
    /// Stored values do not satisfy the current cache contract.
    #[error("the account cache contains invalid data")]
    InvalidData,
    /// The requested account does not own the cache.
    #[error("the account cache belongs to another account")]
    AccountMismatch,
}

impl From<MailCacheError> for AccountCacheOpenError {
    fn from(error: MailCacheError) -> Self {
        match error {
            MailCacheError::Unavailable
            | MailCacheError::MessageNotCached
            | MailCacheError::SyncStateChanged
            | MailCacheError::SendAlreadyPending
            | MailCacheError::PendingSendMismatch
            | MailCacheError::PendingSendNotFound => Self::Unavailable,
            MailCacheError::InUse => Self::InUse,
            MailCacheError::MissingEncryptionKey => Self::MissingEncryptionKey,
            MailCacheError::InvalidData => Self::InvalidData,
            MailCacheError::AccountMismatch => Self::AccountMismatch,
        }
    }
}

impl From<AccountCacheOpenError> for AccountCacheRemovalError {
    fn from(error: AccountCacheOpenError) -> Self {
        match error {
            AccountCacheOpenError::Unavailable => Self::Unavailable,
            AccountCacheOpenError::InUse => Self::InUse,
            AccountCacheOpenError::MissingEncryptionKey => Self::MissingEncryptionKey,
            AccountCacheOpenError::InvalidData => Self::InvalidData,
            AccountCacheOpenError::AccountMismatch => Self::AccountMismatch,
        }
    }
}

/// Account-wide cache removal outcomes shared by mail and calendar data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AccountCacheRemovalError {
    /// The encrypted cache or credential service is unavailable.
    #[error("the account cache is unavailable")]
    Unavailable,
    /// Another cache handle or lifecycle operation owns an account lock.
    #[error("the account cache is in use")]
    InUse,
    /// The account cache exists but its encryption key is missing or malformed.
    #[error("the account cache encryption key is unavailable")]
    MissingEncryptionKey,
    /// Stored values do not satisfy the current cache contract.
    #[error("the account cache contains invalid data")]
    InvalidData,
    /// The requested account does not own the cache.
    #[error("the account cache belongs to another account")]
    AccountMismatch,
    /// An unresolved mail send must be resolved before removal.
    #[error("a mail send needs recovery before account removal")]
    PendingMailSend,
    /// An unresolved calendar event creation must be resolved before removal.
    #[error("a calendar event creation needs recovery before account removal")]
    PendingCalendarEventCreation,
    /// Another account operation currently owns the shared account reservation.
    #[error("an account operation is still active")]
    OperationInProgress,
}

impl From<MailCacheError> for AccountCacheRemovalError {
    fn from(error: MailCacheError) -> Self {
        match error {
            MailCacheError::SendAlreadyPending => Self::PendingMailSend,
            error => AccountCacheOpenError::from(error).into(),
        }
    }
}

impl From<crate::shared::reservations::AccountReservationError> for AccountCacheRemovalError {
    fn from(error: crate::shared::reservations::AccountReservationError) -> Self {
        match error {
            crate::shared::reservations::AccountReservationError::OperationInProgress => {
                Self::OperationInProgress
            }
            crate::shared::reservations::AccountReservationError::StateUnavailable => {
                Self::Unavailable
            }
        }
    }
}

/// Holds the shared account reservation until cache removal completes.
pub(crate) async fn remove_reserved_account_cache<C: AccountCache>(
    cache: &C,
    reservations: &OperationReservations,
    account_id: &AccountId,
) -> Result<(), AccountCacheRemovalError> {
    let _reservation = reservations
        .reserve_account(account_id)
        .map_err(AccountCacheRemovalError::from)?;
    cache.remove_account(account_id).await
}

/// Account-wide lifecycle operations for a cache shared by mail and calendar workflows.
pub trait AccountCache: Send + Sync {
    /// Removes the account's encrypted data and its local encryption key.
    ///
    /// # Errors
    ///
    /// Returns an account cache removal error when a cache lock is held, an unresolved mail or
    /// calendar operation needs recovery, or the cache or key cannot be removed.
    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + '_;
}
