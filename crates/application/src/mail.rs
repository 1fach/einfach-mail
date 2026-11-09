//! Mail provider operations and errors used by application workflows.

use std::{future::Future, time::Duration};

use einfach_mail_domain::{
    AccountId, FolderReference, FollowUpFlagStatus, MailDraft, MailFolder, MailReplyDraft,
    MessageBody, MessageDeltaLink, MessageDeltaPage, MessageId, ReadState,
};
use thiserror::Error;

use crate::AccountOperationError;

/// Selects a well-known folder that has a native message action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WellKnownMailFolder {
    /// The default folder used by Outlook's One_Click Archive action.
    Archive,
    /// The folder that holds messages deleted from ordinary folders.
    DeletedItems,
}

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
    /// The connected account does not support the requested mail operation.
    #[error("this account does not support that mail operation")]
    UnsupportedOperation,
    /// The service asked the application to wait before retrying.
    #[error("the mail service is throttling requests")]
    RateLimited {
        /// The advised delay, or `None` when the provider could not represent it.
        retry_after: Option<Duration>,
    },
    /// The message change may have reached the provider without exact confirmation.
    #[error("the message update outcome is unknown")]
    MessageUpdateOutcomeUnknown,
    /// The Linux credential service could not read or rotate a token.
    #[error("the Linux credential service is unavailable")]
    CredentialStoreUnavailable,
    /// The provider could not complete the network request.
    #[error("the mail service is unavailable")]
    ServiceUnavailable,
    /// The provider returned data that cannot be mapped to mail domain values.
    #[error("the mail service returned invalid data")]
    InvalidResponse,
    /// The provider may have moved a message but did not return exact confirmation.
    #[error("the message move outcome is unknown")]
    MoveOutcomeUnknown,
    /// The response or traversal exceeded the provider's bounded work limits.
    #[error("the mail request exceeded its resource limits")]
    ResourceLimitExceeded,
}

/// A send failure classified by whether Graph could have accepted the message.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MailSendProviderError {
    /// The requested account has no usable provider session.
    #[error("sign-in is required")]
    SignInRequired,
    /// The requested account does not match the provider session.
    #[error("the message belongs to another account")]
    AccountMismatch,
    /// Microsoft denied delegated permission to send mail.
    #[error("Microsoft denied permission to send mail")]
    PermissionRequired,
    /// The connected account does not support sending mail.
    #[error("this account does not support sending mail")]
    UnsupportedOperation,
    /// Microsoft explicitly rejected the message request.
    #[error("Microsoft rejected the message")]
    Rejected,
    /// Microsoft throttled the send request.
    #[error("Microsoft is throttling message sends")]
    RateLimited {
        /// The advised delay, or `None` when Microsoft omitted a usable value.
        retry_after: Option<Duration>,
    },
    /// A local credential or provider setup failed before the send request.
    #[error("the mail service is unavailable")]
    ServiceUnavailable,
    /// The request may have reached Graph, but the app did not receive confirmation.
    #[error("the send outcome is unknown")]
    OutcomeUnknown,
}

impl From<AccountOperationError> for MailSendProviderError {
    fn from(error: AccountOperationError) -> Self {
        match error {
            AccountOperationError::AccessDenied => Self::PermissionRequired,
            AccountOperationError::UnsupportedOperation => Self::UnsupportedOperation,
            AccountOperationError::AccountMismatch => Self::AccountMismatch,
            AccountOperationError::AccountNotConnected
            | AccountOperationError::SavedAccountIdentityUnavailable
            | AccountOperationError::SignInExpired => Self::SignInRequired,
            AccountOperationError::AccessTokenRejected => Self::SignInRequired,
            AccountOperationError::CredentialStoreUnavailable
            | AccountOperationError::InvalidConnectionDetails
            | AccountOperationError::ProfileUnavailable
            | AccountOperationError::ServiceUnavailable
            | AccountOperationError::SignInFailed
            | AccountOperationError::SignInCancelled
            | AccountOperationError::AccountAlreadyConnected
            | AccountOperationError::AccountOperationInProgress
            | AccountOperationError::AccountStateUnavailable => Self::ServiceUnavailable,
        }
    }
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

    /// Sets one message's read state and returns the state confirmed by the provider.
    ///
    /// Implementations must return [`MailOperationError::MessageUpdateOutcomeUnknown`] when the
    /// provider may have applied the update without confirming the requested state.
    ///
    /// # Errors
    ///
    /// Returns [`MailOperationError::SignInRequired`] without a valid session,
    /// [`MailOperationError::AccountMismatch`] when `folder` belongs to another account,
    /// [`MailOperationError::MessageUpdateOutcomeUnknown`] without exact confirmation, or a
    /// provider error for access denial, throttling, or service failures.
    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_;

    /// Sets a message's follow-up flag and returns the status confirmed by the provider.
    ///
    /// Implementations must use the signed-in account and return
    /// [`MailOperationError::MessageUpdateOutcomeUnknown`] when the response
    /// does not confirm the requested status.
    ///
    /// # Errors
    ///
    /// Returns [`MailOperationError::SignInRequired`] without a valid session,
    /// [`MailOperationError::AccountMismatch`] when `folder` belongs to another account,
    /// [`MailOperationError::MessageUpdateOutcomeUnknown`] without exact confirmation, or a
    /// provider error for access, throttling, or service failures.
    fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_;

    /// Moves one message from `source_folder` into `destination_folder`.
    ///
    /// Implementations confirm the returned message ID and destination and do
    /// not retry a request whose outcome is uncertain.
    ///
    /// # Errors
    ///
    /// Returns [`MailOperationError::AccountMismatch`] when either folder is
    /// outside the signed-in account, [`MailOperationError::MoveOutcomeUnknown`]
    /// when the provider may have moved the message without exact confirmation,
    /// or another provider error when it rejects the request.
    fn move_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_;

    /// Resolves a well-known folder for the signed-in account.
    ///
    /// Implementations use the provider's locale-independent folder identity and reject an
    /// account that does not match the active session. The returned reference has the same
    /// account ID as `account_id`.
    ///
    /// # Errors
    ///
    /// Returns [`MailOperationError::AccountMismatch`] when `account_id` is not the signed-in
    /// account, or another provider error when the folder cannot be resolved.
    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_;

    /// Sends one plain-text message as the signed-in account.
    ///
    /// Implementations must submit exactly one request and return
    /// [`MailSendProviderError::OutcomeUnknown`] when Graph may have accepted
    /// the message without returning confirmation.
    ///
    /// # Errors
    ///
    /// Returns a definitive provider failure when the request was rejected,
    /// or an unknown outcome when acceptance cannot be established.
    fn send_mail(
        &self,
        account_id: &AccountId,
        draft: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_;

    /// Replies to a message in the signed-in account using the source's reply recipients.
    ///
    /// Implementations must reject a source scoped to another account and must
    /// not retry a request with an unknown outcome.
    ///
    /// # Errors
    ///
    /// Returns [`MailSendProviderError::SignInRequired`] when no valid session
    /// exists, [`MailSendProviderError::AccountMismatch`] when either account
    /// does not own the source, [`MailSendProviderError::PermissionRequired`]
    /// when Microsoft denies `Mail.Send`, or [`MailSendProviderError::RateLimited`]
    /// when Microsoft throttles the request. Returns
    /// [`MailSendProviderError::Rejected`] when Graph confirms it did not accept
    /// the reply, [`MailSendProviderError::OutcomeUnknown`] when Graph may have
    /// accepted it without confirmation, or
    /// [`MailSendProviderError::ServiceUnavailable`] when local provider setup
    /// prevents the request.
    fn reply_to_message(
        &self,
        account_id: &AccountId,
        draft: MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_;
}

impl From<AccountOperationError> for MailOperationError {
    fn from(error: AccountOperationError) -> Self {
        match error {
            AccountOperationError::SignInExpired | AccountOperationError::AccessTokenRejected => {
                Self::SignInRequired
            }
            AccountOperationError::AccessDenied => Self::AccessDenied,
            AccountOperationError::UnsupportedOperation => Self::UnsupportedOperation,
            AccountOperationError::CredentialStoreUnavailable => Self::CredentialStoreUnavailable,
            _ => Self::ServiceUnavailable,
        }
    }
}
