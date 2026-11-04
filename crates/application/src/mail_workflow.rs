//! Account-scoped synchronization and cached mail workflows.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use einfach_mail_domain::{
    AccountId, CalendarViewReference, FolderReference, MailDraft, MailFolder, MailMessage,
    MailSendOperationId, MessageBody, MessageDeltaProgress, MessageId, PendingMailSend,
};
use thiserror::Error;

use crate::{
    AccountCacheFactory, AccountSession, AccountSessionFactory, AccountSessionFuture,
    CalendarProvider, CalendarSession, CalendarWorkflow, MailCache, MailCacheError,
    MailOperationError, MailProvider, MailSendProviderError, MailSession, MessageBodyForDisplay,
    MessageDeltaResponse, account_session::AccountSessionWorkflow,
    prepare_message_body_for_display,
};

const INITIAL_SYNC_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);
const MAX_PAGES_PER_FOLDER_ROUND: usize = 1_000;
const MAX_SYNC_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
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
}

/// A send result that needs no further provider request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailSendOutcome {
    /// Graph accepted the message and the local marker was removed.
    Accepted,
    /// Graph accepted the message, but the local marker remains for resolution.
    AcceptedNeedsResolution(PendingMailSend),
    /// Graph may have accepted the message; the marker remains and must not be replayed.
    OutcomeUnknown(PendingMailSend),
}

/// A user-safe error from staging, submitting, or explicitly resolving a send.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum MailSendWorkflowError {
    /// The encrypted local cache could not read or update the operation marker.
    #[error("the local mail cache could not update the send operation")]
    Cache(#[from] MailCacheError),
    /// The provider explicitly rejected the message before accepting it.
    #[error("the mail provider rejected the send operation")]
    Provider(#[from] MailSendProviderError),
    /// The provider rejected the message, but its local marker could not be cleared.
    #[error("the provider rejected the message but the local send marker remains")]
    RejectedNeedsResolution {
        /// The rejected operation that remains in the cache.
        pending: PendingMailSend,
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

/// Coordinates provider reads with account-scoped encrypted cache workflows.
pub struct MailWorkflow<P, C> {
    provider: P,
    cache: C,
    reservations: Arc<Mutex<ActiveOperations>>,
}

/// Creates account sessions and shares operation reservations among them.
pub struct AccountSessionWorkflowFactory<P, F> {
    provider: P,
    cache_factory: F,
    reservations: Arc<Mutex<ActiveOperations>>,
}

impl<P, F> AccountSessionWorkflowFactory<P, F> {
    /// Combines a provider with the factory that opens account caches.
    #[must_use]
    pub fn new(provider: P, cache_factory: F) -> Self {
        Self {
            provider,
            cache_factory,
            reservations: Arc::new(Mutex::new(ActiveOperations::default())),
        }
    }
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
        )
    }

    fn with_reservations(
        provider: P,
        cache: C,
        reservations: Arc<Mutex<ActiveOperations>>,
    ) -> Self {
        Self {
            provider,
            cache,
            reservations,
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
        self.submit_staged_send(account_id, draft, pending).await
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
        draft: MailDraft,
        pending: PendingMailSend,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        match self.provider.send_mail(account_id, draft).await {
            Ok(()) => self.finish_accepted_send(account_id, pending).await,
            Err(MailSendProviderError::OutcomeUnknown) => {
                Ok(MailSendOutcome::OutcomeUnknown(pending))
            }
            Err(error) => {
                match self
                    .cache
                    .clear_pending_send(account_id, pending.operation_id())
                    .await
                {
                    Ok(()) => Err(MailSendWorkflowError::Provider(error)),
                    Err(cache) => Err(MailSendWorkflowError::RejectedNeedsResolution {
                        pending,
                        provider: error,
                        cache,
                    }),
                }
            }
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
        let _reservation = self.reserve_send(account_id).map_err(|error| match error {
            MailSendWorkflowError::OperationInProgress => {
                MailSendResolutionError::OperationInProgress
            }
            MailSendWorkflowError::StateUnavailable => MailSendResolutionError::StateUnavailable,
            _ => MailSendResolutionError::StateUnavailable,
        })?;
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
                    response_bytes = response_bytes
                        .checked_add(page_bytes)
                        .filter(|total| *total <= MAX_SYNC_RESPONSE_BYTES)
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

    async fn read_or_fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<(MailMessage, MessageBody, bool), MailWorkflowError> {
        let _reservation = self.reserve_folder(folder)?;
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

    /// Removes the account's encrypted mail cache and its local encryption key.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account does not own this cache or removal fails.
    pub async fn remove_account_cache(&self, account_id: &AccountId) -> Result<(), MailCacheError> {
        let _reservation =
            reserve_account(&self.reservations, account_id).map_err(MailCacheError::from)?;
        self.cache.remove_account(account_id).await?;
        Ok(())
    }

    fn reserve_folder(
        &self,
        folder: &FolderReference,
    ) -> Result<OperationReservation, MailWorkflowError> {
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| MailWorkflowError::StateUnavailable)?;
        if reservations.accounts.contains(folder.account_id())
            || !reservations.folders.insert(folder.clone())
        {
            return Err(MailWorkflowError::OperationInProgress);
        }
        Ok(OperationReservation {
            reservations: Arc::clone(&self.reservations),
            key: ReservationKey::Folder(folder.clone()),
        })
    }

    fn reserve_account(
        &self,
        account: &AccountId,
    ) -> Result<OperationReservation, MailWorkflowError> {
        reserve_account(&self.reservations, account).map_err(|error| match error {
            AccountReservationError::OperationInProgress => MailWorkflowError::OperationInProgress,
            AccountReservationError::StateUnavailable => MailWorkflowError::StateUnavailable,
        })
    }

    fn reserve_send(
        &self,
        account: &AccountId,
    ) -> Result<OperationReservation, MailSendWorkflowError> {
        self.reserve_account(account).map_err(|error| match error {
            MailWorkflowError::OperationInProgress => MailSendWorkflowError::OperationInProgress,
            MailWorkflowError::StateUnavailable => MailSendWorkflowError::StateUnavailable,
            _ => MailSendWorkflowError::StateUnavailable,
        })
    }
}

impl<P, F> AccountSessionFactory for AccountSessionWorkflowFactory<P, F>
where
    P: MailProvider + CalendarProvider + Clone + 'static,
    F: AccountCacheFactory + 'static,
{
    fn open_session(
        &self,
        account_id: AccountId,
    ) -> AccountSessionFuture<'_, Result<Arc<dyn AccountSession>, MailCacheError>> {
        Box::pin(async move {
            let cache = self.cache_factory.open_for_account(account_id).await?;
            let mail_workflow: Arc<dyn MailSession> = Arc::new(MailWorkflow::with_reservations(
                self.provider.clone(),
                cache.clone(),
                Arc::clone(&self.reservations),
            ));
            let calendar_workflow: Arc<dyn CalendarSession> =
                Arc::new(CalendarWorkflow::with_reservations(
                    self.provider.clone(),
                    cache,
                    Arc::clone(&self.reservations),
                ));
            Ok(Arc::new(AccountSessionWorkflow::new(
                mail_workflow,
                calendar_workflow,
            )) as Arc<dyn AccountSession>)
        })
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), MailCacheError>> {
        Box::pin(async move {
            let _reservation =
                reserve_account(&self.reservations, account_id).map_err(MailCacheError::from)?;
            self.cache_factory.remove_for_account(account_id).await
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AccountReservationError {
    OperationInProgress,
    StateUnavailable,
}

impl From<AccountReservationError> for MailCacheError {
    fn from(error: AccountReservationError) -> Self {
        match error {
            AccountReservationError::OperationInProgress => Self::OperationInProgress,
            AccountReservationError::StateUnavailable => Self::Unavailable,
        }
    }
}

pub(crate) fn reserve_account(
    active: &Arc<Mutex<ActiveOperations>>,
    account: &AccountId,
) -> Result<OperationReservation, AccountReservationError> {
    let mut reservations = active
        .lock()
        .map_err(|_| AccountReservationError::StateUnavailable)?;
    if reservations.accounts.contains(account)
        || reservations
            .folders
            .iter()
            .any(|folder| folder.account_id() == account)
        || reservations
            .calendar_views
            .iter()
            .any(|view| view.account_id() == account)
    {
        return Err(AccountReservationError::OperationInProgress);
    }
    reservations.accounts.insert(account.clone());
    Ok(OperationReservation {
        reservations: Arc::clone(active),
        key: ReservationKey::Account(account.clone()),
    })
}

fn request_cursor(
    request: &crate::MessageDeltaRequest,
) -> Option<einfach_mail_domain::MessageDeltaLink> {
    match request {
        crate::MessageDeltaRequest::Initial => None,
        crate::MessageDeltaRequest::Continue(link) => Some(link.clone()),
    }
}

pub(crate) fn reserve_calendar_view(
    active: &Arc<Mutex<ActiveOperations>>,
    view: &CalendarViewReference,
) -> Result<OperationReservation, AccountReservationError> {
    let mut reservations = active
        .lock()
        .map_err(|_| AccountReservationError::StateUnavailable)?;
    if reservations.accounts.contains(view.account_id())
        || !reservations.calendar_views.insert(view.clone())
    {
        return Err(AccountReservationError::OperationInProgress);
    }
    Ok(OperationReservation {
        reservations: Arc::clone(active),
        key: ReservationKey::CalendarView(view.clone()),
    })
}

#[derive(Default)]
pub(crate) struct ActiveOperations {
    accounts: HashSet<AccountId>,
    folders: HashSet<FolderReference>,
    calendar_views: HashSet<CalendarViewReference>,
}

enum ReservationKey {
    Account(AccountId),
    Folder(FolderReference),
    CalendarView(CalendarViewReference),
}

pub(crate) struct OperationReservation {
    reservations: Arc<Mutex<ActiveOperations>>,
    key: ReservationKey,
}

impl Drop for OperationReservation {
    fn drop(&mut self) {
        if let Ok(mut reservations) = self.reservations.lock() {
            match &self.key {
                ReservationKey::Account(account) => {
                    reservations.accounts.remove(account);
                }
                ReservationKey::Folder(folder) => {
                    reservations.folders.remove(folder);
                }
                ReservationKey::CalendarView(view) => {
                    reservations.calendar_views.remove(view);
                }
            }
        }
    }
}
