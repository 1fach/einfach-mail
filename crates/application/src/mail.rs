//! Mail provider operations and errors used by application workflows.

use std::{future::Future, time::Duration};

use einfach_mail_domain::{
    AccountId, FolderReference, MailFolder, MailMessage, MessageBody, MessageDeltaLink,
    MessageDeltaPage, MessageId,
};
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

/// Selects the initial request or the opaque provider link to resume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageDeltaRequest {
    /// Starts the folder's unfiltered initial delta round.
    Initial,
    /// Resumes from the exact next or completed delta link stored for the folder.
    Continue(MessageDeltaLink),
}

/// One provider delta page and the number of bytes in its HTTP response body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageDeltaPageResponse {
    page: MessageDeltaPage,
    response_bytes: usize,
}

impl MessageDeltaPageResponse {
    /// Creates a page response with its received body size.
    #[must_use]
    pub fn new(page: MessageDeltaPage, response_bytes: usize) -> Self {
        Self {
            page,
            response_bytes,
        }
    }

    /// Returns the number of response-body bytes received from the provider.
    #[must_use]
    pub fn response_bytes(&self) -> usize {
        self.response_bytes
    }

    /// Returns the parsed page and its response size.
    #[must_use]
    pub fn into_parts(self) -> (MessageDeltaPage, usize) {
        (self.page, self.response_bytes)
    }
}

/// A message delta page or a provider-requested full resynchronization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageDeltaResponse {
    /// Contains a page and its received response-body size.
    Page(MessageDeltaPageResponse),
    /// Starts a replacement snapshot from this provider-issued opaque link.
    Reset(MessageDeltaLink),
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

    /// Loads one page of changes for `folder` or reports that its cursor expired.
    ///
    /// `request` is either the initial query or an opaque link returned by a
    /// previous page. The provider validates resumed links before it attaches
    /// the signed-in user's token.
    /// The provider returns a reset link only after validating the provider's
    /// reset response for this folder. A page includes its raw body size so the
    /// application can bound a multi-page sync attempt.
    ///
    /// # Errors
    ///
    /// Returns a user-safe provider error when the session, network, response,
    /// or resource limits prevent loading this page.
    fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_;

    /// Fetches a message body after the application confirms it is cached in `folder`.
    ///
    /// The returned content is untrusted and must be stored encrypted and
    /// sanitized before HTML is rendered.
    ///
    /// # Errors
    ///
    /// Returns a user-safe provider error when the session, network, response,
    /// or resource limits prevent loading the body.
    fn fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_;
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
