//! Account-scoped synchronization and cached mail workflows.

mod actions;
mod read;
mod send;
mod sync;

use std::sync::Arc;

use einfach_mail_domain::{
    AccountId, FolderReference, FollowUpFlagStatus, MailDraft, MailMessage, MailReplyDraft,
    PendingMailSend, ReadState,
};
use thiserror::Error;

use crate::shared::recovery::AccountWorkflowRecovery;
use crate::{
    MailCache, MailCacheError, MailOperationError, MailProvider, MailSendProviderError,
    MessageBodyForDisplay, WellKnownMailFolder,
    shared::reservations::{AccountReservationError, OperationReservation, OperationReservations},
};

/// A user-safe category for a mail workflow failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MailWorkflowError {
    /// The provider could not load folders, a delta page, or a message body.
    #[error("the mail service is unavailable")]
    Provider(#[from] MailOperationError),
    /// The encrypted local cache could not read or commit mail data.
    #[error("the local mail cache is unavailable")]
    Cache(#[from] MailCacheError),
    /// A mail folder or reference belongs to another account than this workflow.
    #[error("the mail operation belongs to another account")]
    AccountMismatch,
    /// The destination is not another visible folder in the same account.
    #[error("the selected destination folder is unavailable")]
    InvalidMoveDestination,
    /// A successful mail sync is required before moving messages for this account.
    #[error("mail must sync successfully before moving messages for this account")]
    AccountSyncRequired,
    /// The message already belongs to the requested well-known folder.
    #[error("the message is already in the requested folder")]
    AlreadyInWellKnownFolder(WellKnownMailFolder),
    /// The requested well-known folder is not visible in the account's folder list.
    #[error("the requested well-known folder is unavailable")]
    WellKnownFolderUnavailable(WellKnownMailFolder),
    /// Another mail operation already owns this account or folder.
    #[error("a mail operation is already active for this account or folder")]
    OperationInProgress,
    /// The provider repeated a page link before completing the current round.
    #[error("the mail service repeated a synchronization cursor")]
    RepeatedCursor,
    /// The provider repeatedly invalidated the folder's delta state.
    #[error("the mail service repeatedly reset the folder synchronization state")]
    RepeatedDeltaReset,
    /// The folder reached its per-attempt page limit; its last checkpoint remains resumable.
    #[error("the folder synchronization reached its per-attempt page limit")]
    PageLimitExceeded,
    /// The current sync attempt reached its response size limit and is resumable.
    #[error("the folder synchronization reached its response size limit")]
    ResponseLimitExceeded,
    /// The clock cannot represent the requested initial cutoff.
    #[error("the system clock cannot represent the initial mail cutoff")]
    ClockUnavailable,
    /// The synchronization reservation could not be read or released.
    #[error("the mail workflow state is unavailable")]
    StateUnavailable,
    /// A previous message update requires a complete folder sync before another write.
    #[error("the folder must sync before another message update")]
    FolderSyncRequired,
}

/// A send result that needs no further provider request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailSendOutcome {
    /// Graph accepted the submission and the local marker was removed.
    Accepted,
    /// Graph accepted the submission, but the local marker remains for resolution.
    AcceptedNeedsResolution(PendingMailSend),
    /// Graph may have accepted the submission; the marker remains and must not be replayed.
    OutcomeUnknown(PendingMailSend),
}

enum MailSubmission {
    NewMessage(MailDraft),
    Reply(MailReplyDraft),
}

/// A user-safe error from staging, submitting, or explicitly resolving a send.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum MailSendWorkflowError {
    /// The encrypted local cache could not read or update the operation marker.
    #[error("the local mail cache could not update the send operation")]
    Cache(#[from] MailCacheError),
    /// The provider rejected the operation or its account did not match.
    #[error("the mail provider rejected the send operation")]
    Provider(#[from] MailSendProviderError),
    /// The source message has been removed from the encrypted account cache.
    #[error("the reply source is no longer available in the local mail cache")]
    ReplySourceNotCached,
    /// The provider rejected the operation, but its local marker could not be cleared.
    #[error("the provider rejected the operation but the local send marker remains")]
    RejectedNeedsResolution {
        /// The rejected operation that remains in the cache.
        pending: Box<PendingMailSend>,
        /// The provider's definitive rejection.
        provider: MailSendProviderError,
        /// The failure that prevented clearing the marker.
        cache: MailCacheError,
    },
    /// The account already has a send operation that needs user resolution.
    #[error("a previous send operation must be resolved first")]
    PendingResolutionRequired(PendingMailSend),
    /// Another send or mail operation already owns the account.
    #[error("another mail operation is in progress")]
    OperationInProgress,
    /// The in-memory send reservation could not be read or updated.
    #[error("the mail send workflow state is unavailable")]
    StateUnavailable,
}

/// A user-safe error from clearing an unresolved send marker.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum MailSendResolutionError {
    /// The encrypted local cache could not read or update the operation marker.
    #[error("the local mail cache could not update the send operation")]
    Cache(#[from] MailCacheError),
    /// Another send operation remains unresolved.
    #[error("a previous send operation must be resolved first")]
    PendingResolutionRequired(PendingMailSend),
    /// Another send or mail operation already owns the account.
    #[error("another mail operation is in progress")]
    OperationInProgress,
    /// The in-memory send reservation could not be read or updated.
    #[error("the mail send workflow state is unavailable")]
    StateUnavailable,
}

/// A cached message and a body prepared for safe native display.
#[derive(Eq, PartialEq)]
pub struct OpenedMessage {
    message: MailMessage,
    body: MessageBodyForDisplay,
    loaded_from_cache: bool,
}

impl OpenedMessage {
    /// Returns the cached message-list metadata.
    #[must_use]
    pub fn message(&self) -> &MailMessage {
        &self.message
    }

    /// Returns the body after the application applies its display policy.
    #[must_use]
    pub fn body(&self) -> &MessageBodyForDisplay {
        &self.body
    }

    /// Returns whether the body was available before this open request.
    #[must_use]
    pub fn loaded_from_cache(&self) -> bool {
        self.loaded_from_cache
    }
}

impl std::fmt::Debug for OpenedMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedMessage")
            .field("message", &"<redacted>")
            .field("body", &self.body)
            .field("loaded_from_cache", &self.loaded_from_cache)
            .finish()
    }
}

/// The completed portion of an account synchronization request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MailSyncReport {
    folder_count: usize,
    page_count: usize,
}

/// Progress emitted when synchronization starts replacing a folder snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailSyncProgress {
    /// Graph invalidated the saved cursor and a fresh folder scan has started.
    Rebuilding,
}

/// Describes the result after Graph confirms a message's requested read state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadStateUpdateOutcome {
    /// Graph and the encrypted cache both contain the requested state.
    ConfirmedAndCached(ReadState),
    /// Graph confirmed the state, but the encrypted cache needs a later sync.
    ConfirmedNeedsSync {
        /// The read state Graph confirmed.
        state: ReadState,
        /// The cache error that prevented the confirmed state from being stored.
        cache_error: MailCacheError,
    },
}

/// Describes the result after the provider confirms a message's follow-up flag status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FollowUpFlagUpdateOutcome {
    /// The provider and encrypted cache contain the requested status.
    ConfirmedAndCached(FollowUpFlagStatus),
    /// The provider confirmed the status, but the cache needs a later folder sync.
    ConfirmedNeedsSync {
        /// The status confirmed by the provider.
        status: FollowUpFlagStatus,
        /// The cache error that prevented the update from being stored.
        cache_error: MailCacheError,
    },
}

/// Describes the local cache state after the provider confirms a message move.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageMoveOutcome {
    /// The provider and encrypted cache both contain the moved message.
    ConfirmedAndCached,
    /// The provider confirmed the move, but a later account sync must repair the cache.
    ConfirmedNeedsSync,
}

/// Describes a confirmed move into a well-known folder and the cache result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WellKnownFolderMoveOutcome {
    destination_folder: FolderReference,
    cache_outcome: MessageMoveOutcome,
}

impl WellKnownFolderMoveOutcome {
    /// Returns the account's resolved destination folder.
    #[must_use]
    pub fn destination_folder(&self) -> &FolderReference {
        &self.destination_folder
    }

    /// Returns the encrypted cache result after the provider confirms the move.
    #[must_use]
    pub fn cache_outcome(&self) -> MessageMoveOutcome {
        self.cache_outcome
    }
}

impl MailSyncReport {
    /// Returns the number of visible folders visited.
    #[must_use]
    pub fn folder_count(self) -> usize {
        self.folder_count
    }

    /// Returns the number of delta pages committed.
    #[must_use]
    pub fn page_count(self) -> usize {
        self.page_count
    }
}

impl From<AccountReservationError> for MailWorkflowError {
    fn from(error: AccountReservationError) -> Self {
        match error {
            AccountReservationError::OperationInProgress => Self::OperationInProgress,
            AccountReservationError::StateUnavailable => Self::StateUnavailable,
        }
    }
}

impl From<AccountReservationError> for MailSendWorkflowError {
    fn from(error: AccountReservationError) -> Self {
        match error {
            AccountReservationError::OperationInProgress => Self::OperationInProgress,
            AccountReservationError::StateUnavailable => Self::StateUnavailable,
        }
    }
}

impl From<AccountReservationError> for MailSendResolutionError {
    fn from(error: AccountReservationError) -> Self {
        match error {
            AccountReservationError::OperationInProgress => Self::OperationInProgress,
            AccountReservationError::StateUnavailable => Self::StateUnavailable,
        }
    }
}

/// Coordinates provider reads with account-scoped encrypted cache workflows.
pub struct MailWorkflow<P, C> {
    account_id: AccountId,
    provider: P,
    cache: C,
    reservations: OperationReservations,
    recovery: Arc<AccountWorkflowRecovery>,
}

impl<P: MailProvider, C: MailCache> MailWorkflow<P, C> {
    /// Creates an account-bound mail workflow backed by a provider and local cache.
    ///
    /// Reservations are local to this workflow. When workflows share a persisted cache,
    /// checkpoint revisions reject delta responses built from older checkpoints.
    /// Mail folders and references must belong to `account_id`.
    #[must_use]
    pub fn new(account_id: AccountId, provider: P, cache: C) -> Self {
        Self::with_reservations(
            account_id,
            provider,
            cache,
            OperationReservations::default(),
            Arc::new(AccountWorkflowRecovery::default()),
        )
    }

    pub(crate) fn with_reservations(
        account_id: AccountId,
        provider: P,
        cache: C,
        reservations: OperationReservations,
        recovery: Arc<AccountWorkflowRecovery>,
    ) -> Self {
        Self {
            account_id,
            provider,
            cache,
            reservations,
            recovery,
        }
    }

    fn ensure_folder_account(&self, folder: &FolderReference) -> Result<(), MailWorkflowError> {
        if folder.account_id() == &self.account_id {
            Ok(())
        } else {
            Err(MailWorkflowError::AccountMismatch)
        }
    }

    /// Atomically reserves every requested folder against account and folder operations.
    fn reserve_folders(
        &self,
        folders: &[FolderReference],
    ) -> Result<OperationReservation, MailWorkflowError> {
        self.reservations
            .reserve_folders(folders)
            .map_err(MailWorkflowError::from)
    }
}
