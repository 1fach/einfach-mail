use std::future::Future;

use einfach_mail_domain::AccountId;
use thiserror::Error;

use crate::MailCacheError;

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
            MailCacheError::Unavailable
            | MailCacheError::MessageNotCached
            | MailCacheError::SyncStateChanged
            | MailCacheError::PendingSendMismatch
            | MailCacheError::PendingSendNotFound => Self::Unavailable,
            MailCacheError::InUse => Self::InUse,
            MailCacheError::MissingEncryptionKey => Self::MissingEncryptionKey,
            MailCacheError::InvalidData => Self::InvalidData,
            MailCacheError::AccountMismatch => Self::AccountMismatch,
            MailCacheError::SendAlreadyPending => Self::PendingMailSend,
        }
    }
}

impl From<crate::operation_reservations::AccountReservationError> for AccountCacheRemovalError {
    fn from(error: crate::operation_reservations::AccountReservationError) -> Self {
        match error {
            crate::operation_reservations::AccountReservationError::OperationInProgress => {
                Self::OperationInProgress
            }
            crate::operation_reservations::AccountReservationError::StateUnavailable => {
                Self::Unavailable
            }
        }
    }
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
