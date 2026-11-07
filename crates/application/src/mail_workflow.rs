//! Account-scoped synchronization and cached mail workflows.

use std::{
    collections::HashSet,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use einfach_mail_domain::{
    AccountId, FolderReference, FollowUpFlagStatus, MailDraft, MailFolder, MailMessage,
    MailReplyDraft, MailSendOperationId, MessageBody, MessageDeltaProgress, MessageId,
    PendingMailSend, ReadState,
};
use thiserror::Error;

use crate::account_recovery::AccountWorkflowRecovery;
use crate::{
    MailCache, MailCacheError, MailOperationError, MailProvider, MailSendProviderError,
    MessageBodyForDisplay, MessageDeltaResponse, WellKnownMailFolder,
    operation_reservations::{
        AccountReservationError, ActiveOperations, OperationReservation, reserve_account,
        reserve_folders,
    },
    prepare_message_body_for_display,
    sync_budget::response_bytes_after,
};

const INITIAL_SYNC_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);
const MAX_PAGES_PER_FOLDER_ROUND: usize = 1_000;

fn mail_folder_is_visible(folders: &[MailFolder], reference: &FolderReference) -> bool {
    folders.iter().any(|folder| folder.reference() == reference)
}
const MAX_DELTA_RESETS_PER_ATTEMPT: usize = 1;

/// A user-safe category for a mail workflow failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MailWorkflowError {
    /// The provider could not load folders, a delta page, or a message body.
    #[error("the mail service is unavailable")]
    Provider(#[from] MailOperationError),
    /// The encrypted local cache could not read or commit mail data.
    #[error("the local mail cache is unavailable")]
    Cache(#[from] MailCacheError),
    /// The destination is not another visible folder in the same account.
    #[error("the selected destination folder is unavailable")]
    InvalidMoveDestination,
    /// A previous successful sync is required in the current account session.
    #[error("mail must sync successfully before moving messages in this session")]
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
    /// The current round reached its page limit and remains resumable.
    #[error("the folder synchronization reached its page limit")]
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
    provider: P,
    cache: C,
    reservations: Arc<Mutex<ActiveOperations>>,
    recovery: Arc<AccountWorkflowRecovery>,
}

impl<P: MailProvider, C: MailCache> MailWorkflow<P, C> {
    /// Creates a standalone mail workflow backed by a provider and local cache.
    ///
    /// Reservations are local to this workflow. When workflows share a persisted cache,
    /// checkpoint revisions reject delta responses built from older checkpoints.
    #[must_use]
    pub fn new(provider: P, cache: C) -> Self {
        Self::with_reservations(
            provider,
            cache,
            Arc::new(Mutex::new(ActiveOperations::default())),
            Arc::new(AccountWorkflowRecovery::default()),
        )
    }

    pub(crate) fn with_reservations(
        provider: P,
        cache: C,
        reservations: Arc<Mutex<ActiveOperations>>,
        recovery: Arc<AccountWorkflowRecovery>,
    ) -> Self {
        Self {
            provider,
            cache,
            reservations,
            recovery,
        }
    }

    /// Returns the account's unresolved send marker without contacting Graph.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account does not own the cache or the
    /// encrypted marker cannot be read.
    pub async fn load_pending_send(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<PendingMailSend>, MailCacheError> {
        self.cache.load_pending_send(account_id).await
    }

    /// Stages and submits one message without retrying ambiguous outcomes.
    ///
    /// The body remains in memory and is never written to the cache. A
    /// confirmed acceptance clears the marker; an unknown response leaves it
    /// for explicit user resolution.
    ///
    /// # Errors
    ///
    /// Returns a provider error for a definitive rejection, a cache error
    /// before dispatch, [`MailSendWorkflowError::PendingResolutionRequired`]
    /// when another send remains unresolved, or
    /// [`MailSendWorkflowError::OperationInProgress`] when the account is busy.
    pub async fn send_message(
        &self,
        account_id: &AccountId,
        draft: MailDraft,
        created_at: SystemTime,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        let _reservation = self.reserve_send(account_id)?;
        self.ensure_no_pending_send(account_id).await?;
        let pending =
            draft.pending_send(account_id.clone(), MailSendOperationId::new(), created_at);
        self.stage_send_marker(&pending).await?;
        self.submit_staged_send(account_id, MailSubmission::NewMessage(draft), pending)
            .await
    }

    /// Stages and submits one plain-text reply to a still-cached message.
    ///
    /// The reply body remains in memory. The encrypted marker records only the
    /// source message metadata; a reply request is never automatically retried.
    ///
    /// # Errors
    ///
    /// Returns [`MailSendWorkflowError::ReplySourceNotCached`] when the source
    /// has left the cache, [`MailSendWorkflowError::Provider`] for an account
    /// mismatch or definitive provider rejection, a cache error before dispatch,
    /// or the same unresolved-operation errors as [`Self::send_message`].
    pub async fn reply_to_message(
        &self,
        account_id: &AccountId,
        draft: MailReplyDraft,
        created_at: SystemTime,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        let _reservation = self.reserve_send(account_id)?;
        if draft.source().account_id() != account_id {
            return Err(MailSendProviderError::AccountMismatch.into());
        }
        self.ensure_no_pending_send(account_id).await?;
        let pending = self.stage_reply_send(&draft, created_at).await?;
        self.submit_staged_send(account_id, MailSubmission::Reply(draft), pending)
            .await
    }

    /// Stores source metadata after confirming the reply source remains cached.
    async fn stage_reply_send(
        &self,
        draft: &MailReplyDraft,
        created_at: SystemTime,
    ) -> Result<PendingMailSend, MailSendWorkflowError> {
        let Some((source, _)) = self
            .cache
            .read_message(draft.source().folder(), draft.source().message_id())
            .await?
        else {
            return Err(MailSendWorkflowError::ReplySourceNotCached);
        };
        let pending = draft.pending_send(
            MailSendOperationId::new(),
            source.metadata().subject(),
            created_at,
        );
        self.stage_send_marker(&pending).await?;
        Ok(pending)
    }

    async fn ensure_no_pending_send(
        &self,
        account_id: &AccountId,
    ) -> Result<(), MailSendWorkflowError> {
        if let Some(pending) = self.cache.load_pending_send(account_id).await? {
            return Err(MailSendWorkflowError::PendingResolutionRequired(pending));
        }
        Ok(())
    }

    async fn stage_send_marker(
        &self,
        pending: &PendingMailSend,
    ) -> Result<(), MailSendWorkflowError> {
        match self.cache.stage_pending_send(pending).await {
            Ok(()) => Ok(()),
            Err(MailCacheError::SendAlreadyPending) => {
                match self.cache.load_pending_send(pending.account_id()).await? {
                    Some(pending) => Err(MailSendWorkflowError::PendingResolutionRequired(pending)),
                    None => Err(MailCacheError::SendAlreadyPending.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn submit_staged_send(
        &self,
        account_id: &AccountId,
        submission: MailSubmission,
        pending: PendingMailSend,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        let provider_result = match submission {
            MailSubmission::NewMessage(draft) => self.provider.send_mail(account_id, draft).await,
            MailSubmission::Reply(draft) => self.provider.reply_to_message(account_id, draft).await,
        };
        match provider_result {
            Ok(()) => self.finish_accepted_send(account_id, pending).await,
            Err(MailSendProviderError::OutcomeUnknown) => {
                Ok(MailSendOutcome::OutcomeUnknown(pending))
            }
            Err(error) => self.finish_rejected_send(account_id, pending, error).await,
        }
    }

    async fn finish_rejected_send(
        &self,
        account_id: &AccountId,
        pending: PendingMailSend,
        error: MailSendProviderError,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        match self
            .cache
            .clear_pending_send(account_id, pending.operation_id())
            .await
        {
            Ok(()) => Err(MailSendWorkflowError::Provider(error)),
            Err(cache) => Err(MailSendWorkflowError::RejectedNeedsResolution {
                pending: Box::new(pending),
                provider: error,
                cache,
            }),
        }
    }

    async fn finish_accepted_send(
        &self,
        account_id: &AccountId,
        pending: PendingMailSend,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        match self
            .cache
            .clear_pending_send(account_id, pending.operation_id())
            .await
        {
            Ok(()) => Ok(MailSendOutcome::Accepted),
            Err(_) => Ok(MailSendOutcome::AcceptedNeedsResolution(pending)),
        }
    }

    /// Clears a matching unresolved marker after explicit user review.
    ///
    /// This method never sends or retries a message.
    ///
    /// # Errors
    ///
    /// Returns a cache error for an account mismatch or failed transaction,
    /// [`MailSendResolutionError::PendingResolutionRequired`] when another
    /// operation is pending, or [`MailSendResolutionError::OperationInProgress`]
    /// while another mail operation owns the account.
    pub async fn resolve_pending_send(
        &self,
        account_id: &AccountId,
        operation_id: MailSendOperationId,
    ) -> Result<(), MailSendResolutionError> {
        let _reservation = self.reserve_send(account_id)?;
        let Some(pending) = self.cache.load_pending_send(account_id).await? else {
            return Ok(());
        };
        if pending.operation_id() != operation_id {
            return Err(MailSendResolutionError::PendingResolutionRequired(pending));
        }
        self.cache
            .clear_pending_send(account_id, operation_id)
            .await?;
        Ok(())
    }

    /// Synchronizes every visible folder and commits each page with its checkpoint.
    ///
    /// `sync_started_at` supplies a deterministic clock value for the initial
    /// 90-day cutoff. Existing folder scopes retain their stored cutoff.
    /// One reservation covers folder enumeration, folder replacement, and every delta request.
    /// The handler receives `Rebuilding` after a replacement checkpoint is saved.
    ///
    /// # Panics
    ///
    /// Panics if `progress_handler` panics.
    ///
    /// # Errors
    ///
    /// Returns a provider, cache, synchronization-state, or resource-limit error. Returns
    /// [`MailWorkflowError::OperationInProgress`] when another operation owns this account.
    pub async fn synchronize_account(
        &self,
        account_id: &AccountId,
        sync_started_at: SystemTime,
        progress_handler: &(dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> Result<MailSyncReport, MailWorkflowError> {
        let _reservation = self.reserve_account(account_id)?;
        self.recovery.mark_mail_sync_required();
        let initial_cutoff = sync_started_at
            .checked_sub(INITIAL_SYNC_WINDOW)
            .ok_or(MailWorkflowError::ClockUnavailable)?;
        let folders = self.provider.list_folders(account_id).await?;
        self.cache
            .ensure_account_sync_scope(account_id, initial_cutoff)
            .await?;
        self.cache
            .replace_folders(account_id, folders.clone())
            .await?;
        let page_count = self
            .sync_folders(&folders, initial_cutoff, progress_handler)
            .await?;
        self.recovery.mark_mail_sync_ready();
        Ok(MailSyncReport {
            folder_count: folders.len(),
            page_count,
        })
    }

    async fn sync_folders(
        &self,
        folders: &[MailFolder],
        initial_cutoff: SystemTime,
        progress_handler: &(dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> Result<usize, MailWorkflowError> {
        let mut page_count = 0;
        for folder in folders {
            page_count += self
                .sync_folder(folder.reference(), initial_cutoff, progress_handler)
                .await?;
        }
        Ok(page_count)
    }

    async fn sync_folder(
        &self,
        folder: &FolderReference,
        initial_cutoff: SystemTime,
        progress_handler: &(dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> Result<usize, MailWorkflowError> {
        let mut state = self.cache.ensure_sync_state(folder, initial_cutoff).await?;
        let mut seen_requests = HashSet::new();
        let mut page_count = 0;
        let mut round_page_count = 0;
        let mut response_bytes = 0_usize;
        let mut reset_count = 0;
        loop {
            let expected_state = state.clone();
            let request = expected_state.request();
            let request_cursor = request_cursor(&request);
            if !seen_requests.insert(request_cursor) {
                return Err(MailWorkflowError::RepeatedCursor);
            }
            match self.provider.message_delta_page(folder, request).await? {
                MessageDeltaResponse::Reset(reset_link) => {
                    state = self
                        .cache
                        .begin_full_resync(folder, &expected_state, reset_link)
                        .await?;
                    progress_handler(MailSyncProgress::Rebuilding);
                    if reset_count == MAX_DELTA_RESETS_PER_ATTEMPT {
                        return Err(MailWorkflowError::RepeatedDeltaReset);
                    }
                    reset_count += 1;
                    seen_requests.clear();
                    round_page_count = 0;
                }
                MessageDeltaResponse::Page(response) => {
                    let (page, page_bytes) = response.into_parts();
                    response_bytes = response_bytes_after(response_bytes, page_bytes)
                        .ok_or(MailWorkflowError::ResponseLimitExceeded)?;
                    if let MessageDeltaProgress::Next(link) = page.progress()
                        && seen_requests.contains(&Some(link.clone()))
                    {
                        return Err(MailWorkflowError::RepeatedCursor);
                    }
                    state = self
                        .cache
                        .commit_delta_page(folder, &expected_state, page, SystemTime::now())
                        .await?;
                    page_count += 1;
                    round_page_count += 1;
                    if state.next_link().is_none() {
                        return Ok(page_count);
                    }
                    if round_page_count == MAX_PAGES_PER_FOLDER_ROUND {
                        return Err(MailWorkflowError::PageLimitExceeded);
                    }
                }
            }
        }
    }

    /// Returns the account's folders currently stored in the cache.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account or encrypted database is unavailable.
    pub async fn list_cached_folders(
        &self,
        account_id: &AccountId,
    ) -> Result<Vec<MailFolder>, MailWorkflowError> {
        Ok(self.cache.list_folders(account_id).await?)
    }

    /// Returns cached message metadata for one account-scoped folder.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the folder is outside the account or stored data is invalid.
    pub async fn list_cached_messages(
        &self,
        folder: &FolderReference,
    ) -> Result<Vec<MailMessage>, MailWorkflowError> {
        Ok(self.cache.list_messages(folder).await?)
    }

    /// Returns the saved sync cutoff and cursor state for one folder.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the folder is outside the account or stored state is invalid.
    pub async fn cached_sync_state(
        &self,
        folder: &FolderReference,
    ) -> Result<Option<crate::MailSyncState>, MailWorkflowError> {
        Ok(self.cache.load_sync_state(folder).await?)
    }

    /// Returns whether a folder recovery marker blocks message writes.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the marker cannot be read.
    pub async fn folder_write_sync_required(
        &self,
        folder: &FolderReference,
    ) -> Result<bool, MailWorkflowError> {
        Ok(self.cache.folder_write_sync_required(folder).await?)
    }

    /// Opens a cached message and fetches its body only when it is not already cached.
    ///
    /// The returned body is safe for the native reader. Raw content remains in the cache.
    ///
    /// # Errors
    ///
    /// Returns provider or cache errors when message data is unavailable.
    ///
    /// Returns [`MailWorkflowError::OperationInProgress`] when another operation owns the account
    /// or folder, or [`MailWorkflowError::StateUnavailable`] when the reservation state cannot be
    /// read.
    pub async fn open_message(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<OpenedMessage, MailWorkflowError> {
        let Some((message, cached_body)) = self.cache.read_message(folder, message_id).await?
        else {
            return Err(MailCacheError::MessageNotCached.into());
        };
        let (message, raw_body, loaded_from_cache) = match cached_body {
            Some(body) => (message, body, true),
            None => self.read_or_fetch_message_body(folder, message_id).await?,
        };
        let body = prepare_message_body_for_display(&raw_body);
        Ok(OpenedMessage {
            message,
            body,
            loaded_from_cache,
        })
    }

    /// Sets a cached message's read state after Graph confirms the requested value.
    ///
    /// The operation commits a recovery marker before dispatching the provider request. A complete
    /// folder delta round clears the marker after an unknown outcome or process exit.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the request is rejected, a cache error when the message is
    /// absent or the marker cannot be committed, [`MailWorkflowError::FolderSyncRequired`] while
    /// an earlier update awaits reconciliation, or [`MailWorkflowError::OperationInProgress`]
    /// when another operation owns the account or folder.
    pub async fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> Result<ReadStateUpdateOutcome, MailWorkflowError> {
        let _reservation = self.prepare_message_write(folder, message_id).await?;

        let confirmed = self
            .confirm_message_update(
                folder,
                requested,
                self.provider
                    .set_message_read_state(folder, message_id, requested),
            )
            .await?;

        self.store_confirmed_read_state(folder, message_id, confirmed)
            .await
    }

    async fn store_confirmed_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        confirmed: ReadState,
    ) -> Result<ReadStateUpdateOutcome, MailWorkflowError> {
        match self
            .cache
            .set_message_read_state(folder, message_id, confirmed)
            .await
        {
            Ok(()) => Ok(ReadStateUpdateOutcome::ConfirmedAndCached(confirmed)),
            Err(cache_error) => Ok(ReadStateUpdateOutcome::ConfirmedNeedsSync {
                state: confirmed,
                cache_error,
            }),
        }
    }

    /// Sets a cached message's follow-up flag status after the provider confirms it.
    ///
    /// A SQLCipher recovery marker is committed before the provider request. A complete folder
    /// delta round removes the marker after an unknown outcome or process exit.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the request is rejected, a cache error when the message is
    /// absent or the marker cannot be committed, [`MailWorkflowError::FolderSyncRequired`] while
    /// an earlier update awaits reconciliation, or [`MailWorkflowError::OperationInProgress`]
    /// when another operation owns the account or folder.
    pub async fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: FollowUpFlagStatus,
    ) -> Result<FollowUpFlagUpdateOutcome, MailWorkflowError> {
        let _reservation = self.prepare_message_write(folder, message_id).await?;
        let confirmed = self
            .confirm_message_update(
                folder,
                requested,
                self.provider
                    .set_message_follow_up_flag_status(folder, message_id, requested),
            )
            .await?;

        match self
            .cache
            .set_message_follow_up_flag_status(folder, message_id, confirmed)
            .await
        {
            Ok(()) => Ok(FollowUpFlagUpdateOutcome::ConfirmedAndCached(confirmed)),
            Err(cache_error) => Ok(FollowUpFlagUpdateOutcome::ConfirmedNeedsSync {
                status: confirmed,
                cache_error,
            }),
        }
    }

    /// Moves a cached message between two visible folders in one account.
    ///
    /// The operation holds both folder reservations through provider dispatch
    /// and the cache transaction. A confirmed provider move remains successful
    /// when the cache transaction fails; the caller then synchronizes.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::InvalidMoveDestination`] for identical,
    /// cross-account, or non-visible folders, a cache error when the folder
    /// tree or source message is unavailable, or a provider error when Graph
    /// does not confirm the move,
    /// or [`MailWorkflowError::OperationInProgress`] when an affected scope is busy.
    pub async fn move_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> Result<MessageMoveOutcome, MailWorkflowError> {
        validate_move_folder_pair(source_folder, destination_folder)?;
        let _reservation =
            self.reserve_folders(&[source_folder.clone(), destination_folder.clone()])?;
        self.ensure_account_sync_ready()?;
        self.ensure_visible_move_folders(
            source_folder,
            destination_folder,
            MailWorkflowError::InvalidMoveDestination,
        )
        .await?;
        self.ensure_move_folders_writable(source_folder, destination_folder)
            .await?;
        self.ensure_cached_message(source_folder, message_id)
            .await?;
        self.confirm_and_cache_move(source_folder, message_id, destination_folder)
            .await
    }

    /// Moves a cached message into a well-known folder in the active account.
    ///
    /// The workflow reserves the source before resolving the destination, then
    /// holds both folder reservations through provider confirmation and cache reconciliation.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountSyncRequired`] before a successful
    /// session sync, [`MailWorkflowError::AlreadyInWellKnownFolder`] when the
    /// source is already the requested folder,
    /// [`MailWorkflowError::WellKnownFolderUnavailable`] when the resolved
    /// destination is not visible in this account,
    /// [`MailWorkflowError::FolderSyncRequired`] when either folder needs
    /// reconciliation, a cache or provider error when the message cannot be
    /// moved, or [`MailWorkflowError::OperationInProgress`] when an affected
    /// scope is busy.
    pub async fn move_message_to_well_known_folder(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination: WellKnownMailFolder,
    ) -> Result<WellKnownFolderMoveOutcome, MailWorkflowError> {
        let _source_reservation = self.reserve_folders(std::slice::from_ref(source_folder))?;
        self.ensure_account_sync_ready()?;
        self.ensure_cached_message(source_folder, message_id)
            .await?;

        let destination_folder = self
            .resolve_well_known_folder(source_folder, destination)
            .await?;

        let _destination_reservation =
            self.reserve_folders(std::slice::from_ref(&destination_folder))?;
        self.ensure_visible_move_folders(
            source_folder,
            &destination_folder,
            MailWorkflowError::WellKnownFolderUnavailable(destination),
        )
        .await?;
        self.ensure_move_folders_writable(source_folder, &destination_folder)
            .await?;
        let cache_outcome = self
            .confirm_and_cache_move(source_folder, message_id, &destination_folder)
            .await?;
        Ok(WellKnownFolderMoveOutcome {
            destination_folder,
            cache_outcome,
        })
    }

    async fn resolve_well_known_folder(
        &self,
        source_folder: &FolderReference,
        destination: WellKnownMailFolder,
    ) -> Result<FolderReference, MailWorkflowError> {
        let destination_folder = self
            .provider
            .well_known_folder(source_folder.account_id(), destination)
            .await?;
        if destination_folder.account_id() != source_folder.account_id() {
            return Err(MailWorkflowError::WellKnownFolderUnavailable(destination));
        }
        if source_folder == &destination_folder {
            return Err(MailWorkflowError::AlreadyInWellKnownFolder(destination));
        }
        Ok(destination_folder)
    }

    async fn ensure_visible_move_folders(
        &self,
        source_folder: &FolderReference,
        destination_folder: &FolderReference,
        destination_unavailable: MailWorkflowError,
    ) -> Result<(), MailWorkflowError> {
        let folders = self.cache.list_folders(source_folder.account_id()).await?;
        if !mail_folder_is_visible(&folders, source_folder) {
            return Err(MailWorkflowError::InvalidMoveDestination);
        }
        if !mail_folder_is_visible(&folders, destination_folder) {
            return Err(destination_unavailable);
        }
        Ok(())
    }

    async fn ensure_cached_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<(), MailWorkflowError> {
        if self
            .cache
            .read_message(source_folder, message_id)
            .await?
            .is_some()
        {
            Ok(())
        } else {
            Err(MailCacheError::MessageNotCached.into())
        }
    }

    async fn ensure_move_folders_writable(
        &self,
        source_folder: &FolderReference,
        destination_folder: &FolderReference,
    ) -> Result<(), MailWorkflowError> {
        for folder in [source_folder, destination_folder] {
            if self.cache.folder_write_sync_required(folder).await? {
                return Err(MailWorkflowError::FolderSyncRequired);
            }
        }
        Ok(())
    }

    fn ensure_account_sync_ready(&self) -> Result<(), MailWorkflowError> {
        if self.recovery.mail_sync_ready() {
            Ok(())
        } else {
            Err(MailWorkflowError::AccountSyncRequired)
        }
    }

    async fn confirm_and_cache_move(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> Result<MessageMoveOutcome, MailWorkflowError> {
        let mut reconciliation = MoveReconciliationGuard::new(&self.recovery);
        if let Err(error) = self
            .provider
            .move_message(source_folder, message_id, destination_folder)
            .await
        {
            if error != MailOperationError::MoveOutcomeUnknown {
                reconciliation.complete();
            }
            return Err(error.into());
        }
        match self
            .cache
            .move_message(source_folder, destination_folder, message_id)
            .await
        {
            Ok(()) => {
                reconciliation.complete();
                Ok(MessageMoveOutcome::ConfirmedAndCached)
            }
            Err(_) => Ok(MessageMoveOutcome::ConfirmedNeedsSync),
        }
    }

    async fn prepare_message_write(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<OperationReservation, MailWorkflowError> {
        let reservation = self.reserve_folders(std::slice::from_ref(folder))?;
        if self.cache.folder_write_sync_required(folder).await? {
            return Err(MailWorkflowError::FolderSyncRequired);
        }
        if self
            .cache
            .message_read_state(folder, message_id)
            .await?
            .is_none()
        {
            return Err(MailCacheError::MessageNotCached.into());
        }
        self.cache.require_folder_write_sync(folder).await?;
        Ok(reservation)
    }

    async fn confirm_message_update<T, F>(
        &self,
        folder: &FolderReference,
        requested: T,
        provider_update: F,
    ) -> Result<T, MailWorkflowError>
    where
        T: Copy + Eq + Send,
        F: Future<Output = Result<T, MailOperationError>> + Send,
    {
        let confirmed = match provider_update.await {
            Ok(confirmed) => confirmed,
            Err(MailOperationError::MessageUpdateOutcomeUnknown) => {
                return Err(MailOperationError::MessageUpdateOutcomeUnknown.into());
            }
            Err(error) => {
                self.cache
                    .clear_folder_write_sync_requirement(folder)
                    .await?;
                return Err(error.into());
            }
        };
        (confirmed == requested)
            .then_some(confirmed)
            .ok_or_else(|| MailOperationError::MessageUpdateOutcomeUnknown.into())
    }

    async fn read_or_fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<(MailMessage, MessageBody, bool), MailWorkflowError> {
        let _reservation = self.reserve_folders(std::slice::from_ref(folder))?;
        let Some((message, cached_body)) = self.cache.read_message(folder, message_id).await?
        else {
            return Err(MailCacheError::MessageNotCached.into());
        };
        if let Some(body) = cached_body {
            return Ok((message, body, true));
        }
        let body = self.provider.fetch_message_body(folder, message_id).await?;
        self.cache
            .store_message_body(folder, message_id, body.clone())
            .await?;
        Ok((message, body, false))
    }

    /// Atomically reserves every requested folder against account and folder operations.
    fn reserve_folders(
        &self,
        folders: &[FolderReference],
    ) -> Result<OperationReservation, MailWorkflowError> {
        reserve_folders(&self.reservations, folders).map_err(MailWorkflowError::from)
    }

    fn reserve_account(
        &self,
        account: &AccountId,
    ) -> Result<OperationReservation, MailWorkflowError> {
        reserve_account(&self.reservations, account).map_err(MailWorkflowError::from)
    }

    fn reserve_send(
        &self,
        account: &AccountId,
    ) -> Result<OperationReservation, AccountReservationError> {
        reserve_account(&self.reservations, account)
    }
}

fn request_cursor(
    request: &crate::MessageDeltaRequest,
) -> Option<einfach_mail_domain::MessageDeltaLink> {
    match request {
        crate::MessageDeltaRequest::Initial => None,
        crate::MessageDeltaRequest::Continue(link) => Some(link.clone()),
    }
}

fn validate_move_folder_pair(
    source_folder: &FolderReference,
    destination_folder: &FolderReference,
) -> Result<(), MailWorkflowError> {
    if source_folder.account_id() == destination_folder.account_id()
        && source_folder != destination_folder
    {
        Ok(())
    } else {
        Err(MailWorkflowError::InvalidMoveDestination)
    }
}

/// Marks the session unsynchronized when a move ends without a definitive outcome.
#[must_use = "dropping an unresolved move guard blocks message moves until sync"]
struct MoveReconciliationGuard<'a> {
    recovery: &'a AccountWorkflowRecovery,
    complete: bool,
}

impl<'a> MoveReconciliationGuard<'a> {
    fn new(recovery: &'a AccountWorkflowRecovery) -> Self {
        Self {
            recovery,
            complete: false,
        }
    }

    fn complete(&mut self) {
        self.complete = true;
    }
}

impl Drop for MoveReconciliationGuard<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.recovery.mark_mail_sync_required();
        }
    }
}
