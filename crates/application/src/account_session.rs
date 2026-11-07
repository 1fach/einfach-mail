//! Account-scoped application operations exposed to desktop views.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    time::SystemTime,
};

use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventUpdate, CalendarOccurrence,
    CalendarViewReference, EditableCalendarEvent, FolderReference, FollowUpFlagStatus, MailDraft,
    MailFolder, MailMessage, MailReplyDraft, MailSendOperationId, MessageId, PendingMailSend,
    ReadState,
};

use crate::account_recovery::AccountWorkflowRecovery;
use crate::operation_reservations::{ActiveOperations, reserve_account};
use crate::{
    AccountCache, AccountCacheOpenError, AccountCacheRemovalError, CalendarCache,
    CalendarEventCreationOutcome, CalendarEventDeletionOutcome, CalendarEventUpdateOutcome,
    CalendarProvider, CalendarSnapshot, CalendarSyncReport, CalendarWorkflow,
    CalendarWorkflowError, FollowUpFlagUpdateOutcome, MailCache, MailCacheError, MailProvider,
    MailSendOutcome, MailSendResolutionError, MailSendWorkflowError, MailSyncProgress,
    MailSyncReport, MailSyncState, MailWorkflow, MailWorkflowError, MessageMoveOutcome,
    OpenedMessage, ReadStateUpdateOutcome, WellKnownFolderMoveOutcome, WellKnownMailFolder,
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

    /// Returns whether a folder requires a complete successful sync before another message write.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the folder marker cannot be read.
    fn folder_write_sync_required<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<bool, MailWorkflowError>>;

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

    /// Sets the selected message's read state and returns its provider and cache result.
    ///
    /// # Errors
    ///
    /// Returns provider or cache errors when the requested state cannot be confirmed, or
    /// [`MailWorkflowError::OperationInProgress`] when another operation owns the folder.
    fn set_message_read_state<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
        requested: ReadState,
    ) -> AccountSessionFuture<'a, Result<ReadStateUpdateOutcome, MailWorkflowError>>;

    /// Sets a cached message's follow-up flag and reports whether its confirmed status was cached.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the status cannot be confirmed, a cache error when the
    /// message or recovery marker cannot be stored, [`MailWorkflowError::FolderSyncRequired`]
    /// while an earlier update awaits synchronization, or
    /// [`MailWorkflowError::OperationInProgress`] when another operation owns the account or
    /// folder.
    fn set_message_follow_up_flag_status<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
        status: FollowUpFlagStatus,
    ) -> AccountSessionFuture<'a, Result<FollowUpFlagUpdateOutcome, MailWorkflowError>>;

    /// Moves a cached message to another visible folder in the same account.
    ///
    /// # Errors
    ///
    /// Returns a provider or cache error when the move cannot be confirmed,
    /// [`MailWorkflowError::InvalidMoveDestination`] for a folder outside the
    /// cached visible tree, or [`MailWorkflowError::OperationInProgress`] when
    /// another operation owns either folder or the account.
    fn move_message<'a>(
        &'a self,
        source_folder: &'a FolderReference,
        message_id: &'a MessageId,
        destination_folder: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<MessageMoveOutcome, MailWorkflowError>>;

    /// Moves a cached message into a well-known folder in the signed-in account.
    ///
    /// The workflow requires a successful account sync and leaves the cached
    /// source untouched unless the provider confirms the move.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AlreadyInWellKnownFolder`] when the source is already the
    /// requested folder, [`MailWorkflowError::WellKnownFolderUnavailable`] when the destination
    /// is not visible in the active account, [`MailWorkflowError::InvalidMoveDestination`] when
    /// the source is no longer visible, [`MailWorkflowError::AccountSyncRequired`] before a
    /// successful sync for the account, [`MailWorkflowError::FolderSyncRequired`] when either
    /// folder needs reconciliation, or another provider, cache, or reservation error when the
    /// move cannot complete.
    fn move_message_to_well_known_folder<'a>(
        &'a self,
        source_folder: &'a FolderReference,
        message_id: &'a MessageId,
        destination: WellKnownMailFolder,
    ) -> AccountSessionFuture<'a, Result<WellKnownFolderMoveOutcome, MailWorkflowError>>;

    /// Sends a plain-text message and saves a body-free marker before dispatch.
    fn send_message<'a>(
        &'a self,
        account_id: &'a AccountId,
        draft: MailDraft,
        created_at: SystemTime,
    ) -> AccountSessionFuture<'a, Result<MailSendOutcome, MailSendWorkflowError>>;

    /// Replies to a cached message and saves a body-free marker before dispatch.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the source belongs to another account or
    /// Graph rejects the reply, a cache error before dispatch, or an unresolved
    /// operation error when another send needs explicit resolution.
    fn reply_to_message<'a>(
        &'a self,
        account_id: &'a AccountId,
        draft: MailReplyDraft,
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

    /// Loads and validates the current Graph version of a cached event for editing.
    ///
    /// The account reservation ends when this method returns so the user can
    /// edit the form without blocking synchronization.
    ///
    /// # Errors
    ///
    /// Returns a workflow error when the event cannot be loaded, does not meet
    /// the supported edit rules, or another account operation is active.
    fn editable_event<'a>(
        &'a self,
        event: &'a CalendarOccurrence,
    ) -> AccountSessionFuture<'a, Result<EditableCalendarEvent, CalendarWorkflowError>>;

    /// Creates a staged event and caches it after Graph confirms the result.
    ///
    /// # Errors
    ///
    /// Returns a workflow error when creation could not be staged or when
    /// another account operation is active. Unknown provider outcomes keep the
    /// same operation available for recovery. A definitive rejection and
    /// cleanup failure are returned as
    /// [`CalendarEventCreationOutcome::RejectedButPending`].
    fn create_event<'a>(
        &'a self,
        creation: &'a CalendarEventCreation,
    ) -> AccountSessionFuture<'a, Result<CalendarEventCreationOutcome, CalendarWorkflowError>>;

    /// Clears a creation after Graph definitively rejected it.
    ///
    /// # Errors
    ///
    /// Returns a workflow error when the rejection cannot be recorded or the
    /// local marker cannot be removed.
    fn discard_rejected_event_creation<'a>(
        &'a self,
        creation: &'a CalendarEventCreation,
    ) -> AccountSessionFuture<'a, Result<(), CalendarWorkflowError>>;

    /// Updates one event and atomically reconciles its cached calendar views.
    ///
    /// # Errors
    ///
    /// Returns a workflow error when Graph rejects or cannot confirm the update,
    /// the event changes, or another account operation is active. A cache failure
    /// after Graph confirms the update is returned as
    /// [`CalendarEventUpdateOutcome::ConfirmedButNotCached`].
    fn update_event<'a>(
        &'a self,
        update: &'a CalendarEventUpdate,
    ) -> AccountSessionFuture<'a, Result<CalendarEventUpdateOutcome, CalendarWorkflowError>>;

    /// Deletes one confirmed standalone event and removes every cached view copy.
    ///
    /// # Errors
    ///
    /// Returns a workflow error when Graph rejects the deletion, the event is
    /// stale, another operation owns the account, or a successful sync is
    /// required. A cache failure after Graph confirms deletion is returned as
    /// [`CalendarEventDeletionOutcome::DeletedButNotCached`].
    fn delete_event<'a>(
        &'a self,
        event: &'a EditableCalendarEvent,
    ) -> AccountSessionFuture<'a, Result<CalendarEventDeletionOutcome, CalendarWorkflowError>>;
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
    /// Returns an account-cache removal error when active work, an unresolved operation, or a
    /// cache failure prevents removal.
    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), AccountCacheRemovalError>>;
}

/// Opens one account session and removes account caches when no session is open.
pub trait AccountSessionFactory: Send + Sync {
    /// Opens a session over the account's shared mail and calendar cache.
    ///
    /// Sessions opened by this factory for the same account share operation
    /// reservations and in-memory write-recovery state.
    ///
    /// # Errors
    ///
    /// Returns [`AccountCacheOpenError::Unavailable`] when cache initialization or shared recovery
    /// state is unavailable. Returns [`AccountCacheOpenError::InUse`] when another lifecycle
    /// operation holds the account lock. Returns [`AccountCacheOpenError::MissingEncryptionKey`]
    /// when the cache key is missing, [`AccountCacheOpenError::InvalidData`] when stored cache
    /// values are invalid, and [`AccountCacheOpenError::AccountMismatch`] when the cache belongs
    /// to another account.
    fn open_session(
        &self,
        account_id: AccountId,
    ) -> AccountSessionFuture<'_, Result<Arc<dyn AccountSession>, AccountCacheOpenError>>;

    /// Removes an account cache when no open session is available to remove it.
    ///
    /// # Errors
    ///
    /// Returns an account-cache removal error when another session owns the cache, an unresolved
    /// operation needs recovery, or the cache or key cannot be removed.
    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), AccountCacheRemovalError>>;
}

/// Opens a cloneable handle to one account's shared mail and calendar cache.
pub trait AccountCacheFactory: Send + Sync {
    /// A cache whose clones continue to refer to the same account data.
    type Cache: MailCache + CalendarCache + AccountCache + Clone + 'static;

    /// Opens or creates the cache owned by `account_id`.
    ///
    /// # Errors
    ///
    /// Returns [`AccountCacheOpenError::Unavailable`] when cache or credential access fails.
    /// Returns [`AccountCacheOpenError::InUse`] when another lifecycle operation holds the
    /// account lock. Returns [`AccountCacheOpenError::MissingEncryptionKey`] when an existing
    /// cache has no usable key, [`AccountCacheOpenError::InvalidData`] when stored values are
    /// invalid, and [`AccountCacheOpenError::AccountMismatch`] when the cache belongs to another
    /// account.
    fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = Result<Self::Cache, AccountCacheOpenError>> + Send + '_;

    /// Removes the cache and encryption key owned by `account_id`.
    ///
    /// # Errors
    ///
    /// Returns an account-cache removal error when another session owns the cache, an unresolved
    /// operation needs recovery, or the cache or key cannot be removed.
    fn remove_for_account<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + 'a;
}

/// Creates account sessions and shares operation reservations and write-recovery state among
/// sessions opened by this factory for the same account.
pub struct AccountSessionWorkflowFactory<P, F> {
    provider: P,
    cache_factory: F,
    reservations: Arc<Mutex<ActiveOperations>>,
    recovery_by_account: Mutex<HashMap<AccountId, Weak<AccountWorkflowRecovery>>>,
}

impl<P, F> AccountSessionWorkflowFactory<P, F> {
    /// Combines a provider with the factory that opens account caches.
    #[must_use]
    pub fn new(provider: P, cache_factory: F) -> Self {
        Self {
            provider,
            cache_factory,
            reservations: Arc::new(Mutex::new(ActiveOperations::default())),
            recovery_by_account: Mutex::new(HashMap::new()),
        }
    }

    fn recovery_for_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Arc<AccountWorkflowRecovery>, AccountCacheOpenError> {
        let mut recovery_by_account = self
            .recovery_by_account
            .lock()
            .map_err(|_| AccountCacheOpenError::Unavailable)?;
        recovery_by_account.retain(|_, recovery| recovery.strong_count() > 0);
        if let Some(recovery) = recovery_by_account.get(account_id).and_then(Weak::upgrade) {
            return Ok(recovery);
        }

        let recovery = Arc::new(AccountWorkflowRecovery::default());
        recovery_by_account.insert(account_id.clone(), Arc::downgrade(&recovery));
        Ok(recovery)
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
    ) -> AccountSessionFuture<'_, Result<Arc<dyn AccountSession>, AccountCacheOpenError>> {
        Box::pin(async move {
            let cache = self
                .cache_factory
                .open_for_account(account_id.clone())
                .await?;
            let recovery = self.recovery_for_account(&account_id)?;
            let mail_workflow: Arc<dyn MailSession> = Arc::new(MailWorkflow::with_reservations(
                self.provider.clone(),
                cache.clone(),
                Arc::clone(&self.reservations),
                Arc::clone(&recovery),
            ));
            let calendar_workflow: Arc<dyn CalendarSession> =
                Arc::new(CalendarWorkflow::with_reservations(
                    self.provider.clone(),
                    cache.clone(),
                    Arc::clone(&self.reservations),
                    recovery,
                ));
            Ok(Arc::new(AccountSessionWorkflow::new(
                mail_workflow,
                calendar_workflow,
                cache,
                Arc::clone(&self.reservations),
            )) as Arc<dyn AccountSession>)
        })
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), AccountCacheRemovalError>> {
        Box::pin(async move {
            let _reservation = reserve_account(&self.reservations, account_id)
                .map_err(AccountCacheRemovalError::from)?;
            self.cache_factory.remove_for_account(account_id).await
        })
    }
}

pub(crate) struct AccountSessionWorkflow<C> {
    mail: Arc<dyn MailSession>,
    calendar: Arc<dyn CalendarSession>,
    cache: C,
    reservations: Arc<Mutex<ActiveOperations>>,
}

impl<C> AccountSessionWorkflow<C> {
    pub(crate) fn new(
        mail: Arc<dyn MailSession>,
        calendar: Arc<dyn CalendarSession>,
        cache: C,
        reservations: Arc<Mutex<ActiveOperations>>,
    ) -> Self {
        Self {
            mail,
            calendar,
            cache,
            reservations,
        }
    }
}

impl<C> AccountSession for AccountSessionWorkflow<C>
where
    C: AccountCache,
{
    fn mail_session(&self) -> Arc<dyn MailSession> {
        Arc::clone(&self.mail)
    }

    fn calendar_session(&self) -> Arc<dyn CalendarSession> {
        Arc::clone(&self.calendar)
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), AccountCacheRemovalError>> {
        Box::pin(crate::account_cache::remove_reserved_account_cache(
            &self.cache,
            &self.reservations,
            account_id,
        ))
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

    fn folder_write_sync_required<'a>(
        &'a self,
        folder: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<bool, MailWorkflowError>> {
        Box::pin(MailWorkflow::folder_write_sync_required(self, folder))
    }

    fn open_message<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
    ) -> AccountSessionFuture<'a, Result<OpenedMessage, MailWorkflowError>> {
        Box::pin(MailWorkflow::open_message(self, folder, message_id))
    }

    fn set_message_read_state<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
        requested: ReadState,
    ) -> AccountSessionFuture<'a, Result<ReadStateUpdateOutcome, MailWorkflowError>> {
        Box::pin(MailWorkflow::set_message_read_state(
            self, folder, message_id, requested,
        ))
    }

    fn set_message_follow_up_flag_status<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
        status: FollowUpFlagStatus,
    ) -> AccountSessionFuture<'a, Result<FollowUpFlagUpdateOutcome, MailWorkflowError>> {
        Box::pin(MailWorkflow::set_message_follow_up_flag_status(
            self, folder, message_id, status,
        ))
    }

    fn move_message<'a>(
        &'a self,
        source_folder: &'a FolderReference,
        message_id: &'a MessageId,
        destination_folder: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<MessageMoveOutcome, MailWorkflowError>> {
        Box::pin(MailWorkflow::move_message(
            self,
            source_folder,
            message_id,
            destination_folder,
        ))
    }

    fn move_message_to_well_known_folder<'a>(
        &'a self,
        source_folder: &'a FolderReference,
        message_id: &'a MessageId,
        destination: WellKnownMailFolder,
    ) -> AccountSessionFuture<'a, Result<WellKnownFolderMoveOutcome, MailWorkflowError>> {
        Box::pin(MailWorkflow::move_message_to_well_known_folder(
            self,
            source_folder,
            message_id,
            destination,
        ))
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

    fn reply_to_message<'a>(
        &'a self,
        account_id: &'a AccountId,
        draft: MailReplyDraft,
        created_at: SystemTime,
    ) -> AccountSessionFuture<'a, Result<MailSendOutcome, MailSendWorkflowError>> {
        Box::pin(MailWorkflow::reply_to_message(
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

    fn editable_event<'a>(
        &'a self,
        event: &'a CalendarOccurrence,
    ) -> AccountSessionFuture<'a, Result<EditableCalendarEvent, CalendarWorkflowError>> {
        Box::pin(CalendarWorkflow::editable_event(self, event))
    }

    fn create_event<'a>(
        &'a self,
        creation: &'a CalendarEventCreation,
    ) -> AccountSessionFuture<'a, Result<CalendarEventCreationOutcome, CalendarWorkflowError>> {
        Box::pin(CalendarWorkflow::create_event(self, creation))
    }

    fn discard_rejected_event_creation<'a>(
        &'a self,
        creation: &'a CalendarEventCreation,
    ) -> AccountSessionFuture<'a, Result<(), CalendarWorkflowError>> {
        Box::pin(CalendarWorkflow::discard_rejected_event_creation(
            self, creation,
        ))
    }

    fn update_event<'a>(
        &'a self,
        update: &'a CalendarEventUpdate,
    ) -> AccountSessionFuture<'a, Result<CalendarEventUpdateOutcome, CalendarWorkflowError>> {
        Box::pin(CalendarWorkflow::update_event(self, update))
    }

    fn delete_event<'a>(
        &'a self,
        event: &'a EditableCalendarEvent,
    ) -> AccountSessionFuture<'a, Result<CalendarEventDeletionOutcome, CalendarWorkflowError>> {
        Box::pin(CalendarWorkflow::delete_event(self, event))
    }
}

#[cfg(test)]
mod tests {
    use super::AccountSessionWorkflowFactory;
    use einfach_mail_domain::AccountId;

    fn account_id(value: &str) -> AccountId {
        AccountId::try_from(value.to_owned()).expect("synthetic account ID is valid")
    }

    #[test]
    fn workflow_recovery_is_shared_per_account_and_restarts_after_last_session_drops() {
        let factory = AccountSessionWorkflowFactory::new((), ());
        let account = account_id("synthetic-account");
        let first = factory
            .recovery_for_account(&account)
            .expect("account recovery state is available");
        first.mark_mail_sync_ready();

        let second = factory
            .recovery_for_account(&account)
            .expect("account recovery state is available");
        assert!(second.mail_sync_ready());

        let other_account = factory
            .recovery_for_account(&account_id("other-account"))
            .expect("other account recovery state is available");
        assert!(!other_account.mail_sync_ready());

        drop(first);
        drop(second);
        let reopened = factory
            .recovery_for_account(&account)
            .expect("new account recovery state is available");
        assert!(!reopened.mail_sync_ready());
    }
}
