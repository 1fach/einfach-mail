//! Account-scoped synchronization and cached mail workflows.

use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use einfach_mail_domain::{
    AccountId, FolderReference, MailFolder, MailMessage, MessageDeltaProgress, MessageId,
};
use thiserror::Error;

use crate::{
    MailCache, MailCacheError, MailCacheFactory, MailOperationError, MailProvider, MailSyncState,
    MessageBodyForDisplay, prepare_message_body_for_display,
};

const INITIAL_SYNC_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);
const MAX_PAGES_PER_FOLDER_ROUND: usize = 1_000;
const MAX_SYNC_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// A user-safe category for a mail workflow failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MailWorkflowError {
    /// The provider could not load folders, a delta page, or a message body.
    #[error("the mail service is unavailable")]
    Provider(#[from] MailOperationError),
    /// The encrypted local cache could not read or commit mail data.
    #[error("the local mail cache is unavailable")]
    Cache(#[from] MailCacheError),
    /// Another synchronization round already owns this account and folder.
    #[error("this folder is already synchronizing")]
    SyncInProgress,
    /// The provider repeated a page link before completing the current round.
    #[error("the mail service repeated a synchronization cursor")]
    RepeatedCursor,
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

/// A boxed future returned by a type-erased mail session.
pub type MailSessionFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Provides mail workflows to consumers that do not own provider or cache types.
pub trait MailSession: Send + Sync {
    /// Synchronizes visible folders and commits each page with its checkpoint.
    ///
    /// # Errors
    ///
    /// Returns provider, cache, synchronization-state, or resource-limit errors.
    fn synchronize_account<'a>(
        &'a self,
        account_id: &'a AccountId,
        sync_started_at: SystemTime,
    ) -> MailSessionFuture<'a, Result<MailSyncReport, MailWorkflowError>>;

    /// Returns the visible folders saved for `account_id`.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account cache is unavailable or invalid.
    fn list_cached_folders<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> MailSessionFuture<'a, Result<Vec<MailFolder>, MailWorkflowError>>;

    /// Returns cached message metadata in newest-first order.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the folder is outside the account or stored data is invalid.
    fn list_cached_messages<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> MailSessionFuture<'a, Result<Vec<MailMessage>, MailWorkflowError>>;

    /// Returns the saved cutoff and cursor state for one folder.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the folder is outside the account or stored state is invalid.
    fn cached_sync_state<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> MailSessionFuture<'a, Result<Option<MailSyncState>, MailWorkflowError>>;

    /// Opens a cached message and fetches its body only when it is not cached.
    ///
    /// The returned body has passed the application's display policy.
    ///
    /// # Errors
    ///
    /// Returns provider or cache errors when metadata, body, or durable storage is unavailable.
    fn open_message<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
    ) -> MailSessionFuture<'a, Result<OpenedMessage, MailWorkflowError>>;

    /// Removes this session's account cache and its local encryption key.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account does not own the cache or removal fails.
    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> MailSessionFuture<'a, Result<(), MailCacheError>>;
}

/// Opens mail sessions and removes account caches when no session is open.
pub trait MailSessionFactory: Send + Sync {
    /// Opens an account-scoped mail session.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account cache or its credential is unavailable.
    fn open_session(
        &self,
        account_id: AccountId,
    ) -> MailSessionFuture<'_, Result<Arc<dyn MailSession>, MailCacheError>>;

    /// Removes an account cache when no open session is available to remove it.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account files or credential cannot be removed.
    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> MailSessionFuture<'a, Result<(), MailCacheError>>;
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
    active_folders: Arc<Mutex<HashSet<FolderReference>>>,
}

/// Creates type-erased application sessions from a provider and a cache factory.
pub struct MailWorkflowFactory<P, F> {
    provider: P,
    cache_factory: F,
}

impl<P, F> MailWorkflowFactory<P, F> {
    /// Combines a provider with the factory that opens account caches.
    #[must_use]
    pub fn new(provider: P, cache_factory: F) -> Self {
        Self {
            provider,
            cache_factory,
        }
    }
}

impl<P: MailProvider, C: MailCache> MailWorkflow<P, C> {
    /// Creates mail workflows backed by a provider and local cache.
    #[must_use]
    pub fn new(provider: P, cache: C) -> Self {
        Self {
            provider,
            cache,
            active_folders: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Synchronizes every visible folder and commits each page with its checkpoint.
    ///
    /// `sync_started_at` supplies a deterministic clock value for the initial
    /// 90-day cutoff. Existing folder scopes retain their stored cutoff.
    ///
    /// # Errors
    ///
    /// Returns a provider, cache, synchronization-state, or resource-limit error.
    pub async fn synchronize_account(
        &self,
        account_id: &AccountId,
        sync_started_at: SystemTime,
    ) -> Result<MailSyncReport, MailWorkflowError> {
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
        let page_count = self.sync_folders(&folders, initial_cutoff).await?;
        Ok(MailSyncReport {
            folder_count: folders.len(),
            page_count,
        })
    }

    async fn sync_folders(
        &self,
        folders: &[MailFolder],
        initial_cutoff: SystemTime,
    ) -> Result<usize, MailWorkflowError> {
        let mut page_count = 0;
        for folder in folders {
            let _reservation = self.reserve_folder(folder.reference())?;
            page_count += self.sync_folder(folder.reference(), initial_cutoff).await?;
        }
        Ok(page_count)
    }

    async fn sync_folder(
        &self,
        folder: &FolderReference,
        initial_cutoff: SystemTime,
    ) -> Result<usize, MailWorkflowError> {
        let mut state = self.cache.ensure_sync_state(folder, initial_cutoff).await?;
        let mut seen_requests = HashSet::new();
        let mut page_count = 0;
        let mut response_bytes = 0_usize;
        loop {
            let request = state.request();
            let request_cursor = request_cursor(&request);
            if !seen_requests.insert(request_cursor) {
                return Err(MailWorkflowError::RepeatedCursor);
            }
            let response = self.provider.message_delta_page(folder, request).await?;
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
                .commit_delta_page(folder, page, SystemTime::now())
                .await?;
            page_count += 1;
            if state.next_link().is_none() {
                return Ok(page_count);
            }
            if page_count == MAX_PAGES_PER_FOLDER_ROUND {
                return Err(MailWorkflowError::PageLimitExceeded);
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
    /// Returns a provider or cache error when metadata, body, or durable storage is unavailable.
    pub async fn open_message(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<OpenedMessage, MailWorkflowError> {
        let Some((message, cached_body)) = self.cache.read_message(folder, message_id).await?
        else {
            return Err(MailCacheError::MessageNotCached.into());
        };
        let (raw_body, loaded_from_cache) = match cached_body {
            Some(body) => (body, true),
            None => {
                let body = self.provider.fetch_message_body(folder, message_id).await?;
                self.cache
                    .store_message_body(folder, message_id, body.clone())
                    .await?;
                (body, false)
            }
        };
        let body = prepare_message_body_for_display(&raw_body);
        Ok(OpenedMessage {
            message,
            body,
            loaded_from_cache,
        })
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
    ) -> Result<SyncReservation, MailWorkflowError> {
        let mut active_folders = self
            .active_folders
            .lock()
            .map_err(|_| MailWorkflowError::StateUnavailable)?;
        if !active_folders.insert(folder.clone()) {
            return Err(MailWorkflowError::SyncInProgress);
        }
        Ok(SyncReservation {
            active_folders: Arc::clone(&self.active_folders),
            folder: folder.clone(),
        })
    }
}

impl<P: MailProvider, C: MailCache> MailSession for MailWorkflow<P, C> {
    fn synchronize_account<'a>(
        &'a self,
        account_id: &'a AccountId,
        sync_started_at: SystemTime,
    ) -> MailSessionFuture<'a, Result<MailSyncReport, MailWorkflowError>> {
        Box::pin(MailWorkflow::synchronize_account(
            self,
            account_id,
            sync_started_at,
        ))
    }

    fn list_cached_folders<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> MailSessionFuture<'a, Result<Vec<MailFolder>, MailWorkflowError>> {
        Box::pin(MailWorkflow::list_cached_folders(self, account_id))
    }

    fn list_cached_messages<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> MailSessionFuture<'a, Result<Vec<MailMessage>, MailWorkflowError>> {
        Box::pin(MailWorkflow::list_cached_messages(self, folder))
    }

    fn cached_sync_state<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> MailSessionFuture<'a, Result<Option<MailSyncState>, MailWorkflowError>> {
        Box::pin(MailWorkflow::cached_sync_state(self, folder))
    }

    fn open_message<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
    ) -> MailSessionFuture<'a, Result<OpenedMessage, MailWorkflowError>> {
        Box::pin(MailWorkflow::open_message(self, folder, message_id))
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> MailSessionFuture<'a, Result<(), MailCacheError>> {
        Box::pin(MailWorkflow::remove_account_cache(self, account_id))
    }
}

impl<P, F> MailSessionFactory for MailWorkflowFactory<P, F>
where
    P: MailProvider + Clone + 'static,
    F: MailCacheFactory + 'static,
{
    fn open_session(
        &self,
        account_id: AccountId,
    ) -> MailSessionFuture<'_, Result<Arc<dyn MailSession>, MailCacheError>> {
        Box::pin(async move {
            let cache = self.cache_factory.open_for_account(account_id).await?;
            Ok(Arc::new(MailWorkflow::new(self.provider.clone(), cache)) as Arc<dyn MailSession>)
        })
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> MailSessionFuture<'a, Result<(), MailCacheError>> {
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

struct SyncReservation {
    active_folders: Arc<Mutex<HashSet<FolderReference>>>,
    folder: FolderReference,
}

impl Drop for SyncReservation {
    fn drop(&mut self) {
        if let Ok(mut active_folders) = self.active_folders.lock() {
            active_folders.remove(&self.folder);
        }
    }
}
