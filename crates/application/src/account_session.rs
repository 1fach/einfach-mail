//! Account-scoped application operations exposed to desktop views.

use std::{future::Future, pin::Pin, sync::Arc, time::SystemTime};

use einfach_mail_domain::{
    AccountId, CalendarViewReference, FolderReference, MailDraft, MailFolder, MailMessage,
    MailSendOperationId, MessageId, PendingMailSend,
};

use crate::{
    CalendarCache, CalendarProvider, CalendarSnapshot, CalendarSyncReport, CalendarWorkflow,
    CalendarWorkflowError, MailCache, MailCacheError, MailProvider, MailSendOutcome,
    MailSendResolutionError, MailSendWorkflowError, MailSyncProgress, MailSyncReport,
    MailSyncState, MailWorkflow, MailWorkflowError, OpenedMessage,
};

/// A boxed future returned by an account-scoped application session.
pub type AccountSessionFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Mail operations for one signed-in account.
pub trait MailSession: Send + Sync {
    /// Synchronizes visible folders and commits each page with its checkpoint.
    /// Calls `progress_handler` after a replacement snapshot checkpoint is saved.
    ///
    /// # Panics
    ///
    /// Panics if `progress_handler` panics.
    ///
    /// # Errors
    ///
    /// Returns provider, cache, synchronization-state, or resource-limit errors.
    fn synchronize_account<'a>(
        &'a self,
        account_id: &'a AccountId,
        sync_started_at: SystemTime,
        progress_handler: &'a (dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> AccountSessionFuture<'a, Result<MailSyncReport, MailWorkflowError>>;

    /// Returns the visible folders saved for `account_id`.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account cache is unavailable or invalid.
    fn list_cached_folders<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<Vec<MailFolder>, MailWorkflowError>>;

    /// Returns cached message metadata in newest-first order.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the folder is outside the account or stored data is invalid.
    fn list_cached_messages<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<Vec<MailMessage>, MailWorkflowError>>;

    /// Returns the saved cutoff and cursor state for one folder.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the folder is outside the account or stored state is invalid.
    fn cached_sync_state<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<Option<MailSyncState>, MailWorkflowError>>;

    /// Opens a cached message and fetches its body only when it is not cached.
    ///
    /// The returned body has passed the application's display policy.
    ///
    /// # Errors
    ///
    /// Returns provider, cache, or synchronization-state errors when data is unavailable or the folder is busy.
    fn open_message<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
    ) -> AccountSessionFuture<'a, Result<OpenedMessage, MailWorkflowError>>;

    /// Sends a plain-text message and saves a body-free marker before dispatch.
    fn send_message<'a>(
        &'a self,
        account_id: &'a AccountId,
        draft: MailDraft,
        created_at: SystemTime,
    ) -> AccountSessionFuture<'a, Result<MailSendOutcome, MailSendWorkflowError>>;

    /// Reads the unresolved send marker without contacting Graph.
    fn load_pending_send<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<Option<PendingMailSend>, MailCacheError>>;

    /// Clears a marker after the user explicitly checks Sent Items.
    fn resolve_pending_send<'a>(
        &'a self,
        account_id: &'a AccountId,
        operation_id: MailSendOperationId,
    ) -> AccountSessionFuture<'a, Result<(), MailSendResolutionError>>;

    /// Removes this session's account cache and its local encryption key.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account does not own the cache or removal fails.
    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), MailCacheError>>;
}

/// Calendar operations for one signed-in account.
pub trait CalendarSession: Send + Sync {
    /// Loads cached occurrences and cursor state for a fixed calendar view.
    ///
    /// # Errors
    ///
    /// Returns a calendar cache error when the view or stored state cannot be read.
    fn load_cached_view<'a>(
        &'a self,
        view: &'a CalendarViewReference,
    ) -> AccountSessionFuture<'a, Result<CalendarSnapshot, CalendarWorkflowError>>;

    /// Synchronizes all delta pages for a fixed calendar view.
    ///
    /// # Errors
    ///
    /// Returns a provider, cache, synchronization-state, or resource-limit error.
    fn synchronize_view<'a>(
        &'a self,
        view: &'a CalendarViewReference,
    ) -> AccountSessionFuture<'a, Result<CalendarSyncReport, CalendarWorkflowError>>;
}

/// Mail and calendar sessions that share one account cache and removal lifecycle.
pub trait AccountSession: Send + Sync {
    /// Returns the mail operations for this account.
    fn mail_session(&self) -> Arc<dyn MailSession>;

    /// Returns the calendar operations for this account.
    fn calendar_session(&self) -> Arc<dyn CalendarSession>;

    /// Removes this account's cache and its local encryption key.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account does not own the cache or removal fails.
    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), MailCacheError>>;
}

/// Opens one account session and removes account caches when no session is open.
pub trait AccountSessionFactory: Send + Sync {
    /// Opens a session over the account's shared mail and calendar cache.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account cache or its credential is unavailable.
    fn open_session(
        &self,
        account_id: AccountId,
    ) -> AccountSessionFuture<'_, Result<Arc<dyn AccountSession>, MailCacheError>>;

    /// Removes an account cache when no open session is available to remove it.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account files or credential cannot be removed.
    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), MailCacheError>>;
}

/// Opens a cloneable handle to one account's shared mail and calendar cache.
pub trait AccountCacheFactory: Send + Sync {
    /// A cache whose clones continue to refer to the same account data.
    type Cache: MailCache + CalendarCache + Clone + 'static;

    /// Opens or creates the cache owned by `account_id`.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account cache or its credential is unavailable.
    fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = Result<Self::Cache, MailCacheError>> + Send + '_;

    /// Removes the cache and encryption key owned by `account_id`.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account files or credential cannot be removed.
    fn remove_for_account<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + 'a;
}

pub(crate) struct AccountSessionWorkflow {
    mail: Arc<dyn MailSession>,
    calendar: Arc<dyn CalendarSession>,
}

impl AccountSessionWorkflow {
    pub(crate) fn new(mail: Arc<dyn MailSession>, calendar: Arc<dyn CalendarSession>) -> Self {
        Self { mail, calendar }
    }
}

impl AccountSession for AccountSessionWorkflow {
    fn mail_session(&self) -> Arc<dyn MailSession> {
        Arc::clone(&self.mail)
    }

    fn calendar_session(&self) -> Arc<dyn CalendarSession> {
        Arc::clone(&self.calendar)
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), MailCacheError>> {
        self.mail.remove_account_cache(account_id)
    }
}

impl<P, C> MailSession for MailWorkflow<P, C>
where
    P: MailProvider,
    C: MailCache,
{
    fn synchronize_account<'a>(
        &'a self,
        account_id: &'a AccountId,
        sync_started_at: SystemTime,
        progress_handler: &'a (dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> AccountSessionFuture<'a, Result<MailSyncReport, MailWorkflowError>> {
        Box::pin(MailWorkflow::synchronize_account(
            self,
            account_id,
            sync_started_at,
            progress_handler,
        ))
    }

    fn list_cached_folders<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<Vec<MailFolder>, MailWorkflowError>> {
        Box::pin(MailWorkflow::list_cached_folders(self, account_id))
    }

    fn list_cached_messages<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<Vec<MailMessage>, MailWorkflowError>> {
        Box::pin(MailWorkflow::list_cached_messages(self, folder))
    }

    fn cached_sync_state<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<Option<MailSyncState>, MailWorkflowError>> {
        Box::pin(MailWorkflow::cached_sync_state(self, folder))
    }

    fn open_message<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
    ) -> AccountSessionFuture<'a, Result<OpenedMessage, MailWorkflowError>> {
        Box::pin(MailWorkflow::open_message(self, folder, message_id))
    }

    fn send_message<'a>(
        &'a self,
        account_id: &'a AccountId,
        draft: MailDraft,
        created_at: SystemTime,
    ) -> AccountSessionFuture<'a, Result<MailSendOutcome, MailSendWorkflowError>> {
        Box::pin(MailWorkflow::send_message(
            self, account_id, draft, created_at,
        ))
    }

    fn load_pending_send<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<Option<PendingMailSend>, MailCacheError>> {
        Box::pin(MailWorkflow::load_pending_send(self, account_id))
    }

    fn resolve_pending_send<'a>(
        &'a self,
        account_id: &'a AccountId,
        operation_id: MailSendOperationId,
    ) -> AccountSessionFuture<'a, Result<(), MailSendResolutionError>> {
        Box::pin(MailWorkflow::resolve_pending_send(
            self,
            account_id,
            operation_id,
        ))
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), MailCacheError>> {
        Box::pin(MailWorkflow::remove_account_cache(self, account_id))
    }
}

impl<P, C> CalendarSession for CalendarWorkflow<P, C>
where
    P: CalendarProvider,
    C: CalendarCache,
{
    fn load_cached_view<'a>(
        &'a self,
        view: &'a CalendarViewReference,
    ) -> AccountSessionFuture<'a, Result<CalendarSnapshot, CalendarWorkflowError>> {
        Box::pin(CalendarWorkflow::load_cached_view(self, view))
    }

    fn synchronize_view<'a>(
        &'a self,
        view: &'a CalendarViewReference,
    ) -> AccountSessionFuture<'a, Result<CalendarSyncReport, CalendarWorkflowError>> {
        Box::pin(CalendarWorkflow::sync_view(self, view))
    }
}
