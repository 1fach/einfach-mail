//! Account-scoped synchronization and cached mail workflows.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use einfach_mail_domain::{
    AccountId, CalendarViewReference, FolderReference, MailFolder, MailMessage, MessageBody,
    MessageDeltaProgress, MessageId,
};
use thiserror::Error;

use crate::{
    AccountCacheFactory, AccountSession, AccountSessionFactory, AccountSessionFuture,
    CalendarProvider, CalendarSession, CalendarWorkflow, MailCache, MailCacheError,
    MailOperationError, MailProvider, MailSession, MessageBodyForDisplay, MessageDeltaResponse,
    account_session::AccountSessionWorkflow, prepare_message_body_for_display,
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
    calendar_reservations: Arc<Mutex<HashSet<CalendarViewReference>>>,
}

impl<P, F> AccountSessionWorkflowFactory<P, F> {
    /// Combines a provider with the factory that opens account caches.
    #[must_use]
    pub fn new(provider: P, cache_factory: F) -> Self {
        Self {
            provider,
            cache_factory,
            reservations: Arc::new(Mutex::new(ActiveOperations::default())),
            calendar_reservations: Arc::new(Mutex::new(HashSet::new())),
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
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| MailWorkflowError::StateUnavailable)?;
        if reservations.accounts.contains(account)
            || reservations
                .folders
                .iter()
                .any(|folder| folder.account_id() == account)
        {
            return Err(MailWorkflowError::OperationInProgress);
        }
        reservations.accounts.insert(account.clone());
        Ok(OperationReservation {
            reservations: Arc::clone(&self.reservations),
            key: ReservationKey::Account(account.clone()),
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
                    Arc::clone(&self.calendar_reservations),
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
        Box::pin(self.cache_factory.remove_for_account(account_id))
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

#[derive(Default)]
struct ActiveOperations {
    accounts: HashSet<AccountId>,
    folders: HashSet<FolderReference>,
}

enum ReservationKey {
    Account(AccountId),
    Folder(FolderReference),
}

struct OperationReservation {
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
            }
        }
    }
}
