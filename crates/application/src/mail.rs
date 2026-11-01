//! Mail provider operations and errors used by application workflows.

use std::{future::Future, time::Duration};

use einfach_mail_domain::{AccountId, FolderReference, MailFolder, MailMessage};
use thiserror::Error;

use crate::AccountOperationError;

/// A user-safe category for a mail operation failure.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MailOperationError {
    /// The requested account has no active provider session.
    #[error("sign-in is required")]
    SignInRequired,
    /// The requested resource belongs to a different connected account.
    #[error("the resource belongs to another account")]
    AccountMismatch,
    /// The provider denied access to the requested mail data.
    #[error("mail access was denied")]
    AccessDenied,
    /// The service asked the application to wait before retrying.
    #[error("the mail service is throttling requests")]
    RateLimited {
        /// The advised delay, or `None` when the provider could not represent it.
        retry_after: Option<Duration>,
    },
    /// The Linux credential service could not read or rotate a token.
    #[error("the Linux credential service is unavailable")]
    CredentialStoreUnavailable,
    /// The provider could not complete the network request.
    #[error("the mail service is unavailable")]
    ServiceUnavailable,
    /// The provider returned data that cannot be mapped to mail domain values.
    #[error("the mail service returned invalid data")]
    InvalidResponse,
    /// The response or traversal exceeded the provider's bounded work limits.
    #[error("the mail request exceeded its resource limits")]
    ResourceLimitExceeded,
}

/// Mail operations the application needs from a connected-account provider.
pub trait MailProvider: Send + Sync {
    /// Lists the provider's folders for `account_id`.
    ///
    /// The returned list contains provider-independent folder values. The
    /// provider must reject an account ID that does not match its active
    /// session.
    ///
    /// # Errors
    ///
    /// Returns [`MailOperationError::SignInRequired`] when no valid session
    /// exists, [`MailOperationError::AccountMismatch`] when `account_id` is
    /// outside that session, or another error for access denial, throttling,
    /// invalid provider data, unavailable services, or exceeded work limits.
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_;

    /// Lists provider-independent message metadata for `folder`.
    ///
    /// The result does not include message bodies. The provider must reject a
    /// folder reference scoped to another account.
    ///
    /// # Errors
    ///
    /// Returns [`MailOperationError::SignInRequired`] when no valid session
    /// exists, [`MailOperationError::AccountMismatch`] when `folder` is
    /// outside that session, or another error for access denial, throttling,
    /// invalid provider data, unavailable services, or exceeded work limits.
    fn list_messages(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_;
}

impl From<AccountOperationError> for MailOperationError {
    fn from(error: AccountOperationError) -> Self {
        match error {
            AccountOperationError::SignInExpired | AccountOperationError::AccessTokenRejected => {
                Self::SignInRequired
            }
            AccountOperationError::AccessDenied => Self::AccessDenied,
            AccountOperationError::CredentialStoreUnavailable => Self::CredentialStoreUnavailable,
            _ => Self::ServiceUnavailable,
        }
    }
}
