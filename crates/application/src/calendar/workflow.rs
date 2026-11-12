//! Calendar synchronization and event workflows for account-scoped views.

use std::{collections::HashSet, sync::Arc, time::SystemTime};

use einfach_mail_domain::{
    AccountId, CalendarDeltaLink, CalendarDeltaPage, CalendarEventCreation, CalendarEventUpdate,
    CalendarOccurrence, CalendarViewReference, EditableCalendarEvent, PendingCalendarEventCreation,
};
use thiserror::Error;

use super::{
    cache::{CalendarCache, CalendarCacheError, CalendarSyncState},
    provider::{
        CalendarDeltaRequest, CalendarDeltaResponse, CalendarOperationError, CalendarProvider,
        CalendarSyncStrategy,
    },
};
use crate::shared::recovery::{AccountWorkflowRecovery, CalendarWriteReadiness};
use crate::{
    shared::reservations::{AccountReservationError, OperationReservations},
    shared::sync_budget::{
        MAX_DELTA_PAGES_PER_ATTEMPT, MAX_DELTA_RESETS_PER_ATTEMPT, response_bytes_after,
    },
};

/// A user-safe category for a calendar workflow failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CalendarWorkflowError {
    /// The provider could not load a calendar synchronization response.
    #[error("the calendar service is unavailable")]
    Provider(#[from] CalendarOperationError),
    /// The encrypted local cache could not read or commit events.
    #[error("the local calendar cache is unavailable")]
    Cache(#[from] CalendarCacheError),
    /// A calendar reference belongs to another account than this workflow.
    #[error("the calendar operation belongs to another account")]
    AccountMismatch,
    /// Another mail or calendar operation already owns this account.
    #[error("another mail or calendar operation is active for this account")]
    OperationInProgress,
    /// Graph did not return the event that was staged for creation.
    #[error("the calendar service returned an unexpected created event")]
    InvalidConfirmation,
    /// Graph did not return the event and values submitted for update.
    #[error("the calendar service returned an unexpected updated event")]
    InvalidUpdateConfirmation,
    /// The provider returned an edit snapshot for another event or account.
    #[error("the calendar service returned an unexpected event for editing")]
    InvalidEditableEvent,
    /// A view sync or account replacement scan must finish before a calendar write.
    #[error("sync the calendar before changing calendar events")]
    CalendarWriteSyncRequired,
    /// The provider repeated a cursor before completing the current round.
    #[error("the calendar service repeated a synchronization cursor")]
    RepeatedCursor,
    /// The current round reached its page limit and remains resumable.
    #[error("calendar synchronization reached its page limit")]
    PageLimitExceeded,
    /// The current sync attempt reached its response size limit and is resumable.
    #[error("calendar synchronization reached its response size limit")]
    ResponseLimitExceeded,
    /// The provider invalidated more than one cursor in this synchronization attempt.
    #[error("calendar synchronization repeatedly reset its cursor")]
    DeltaResetLimitExceeded,
    /// The account reservation or shared recovery state could not be accessed.
    #[error("the calendar workflow state is unavailable")]
    StateUnavailable,
}

impl From<AccountReservationError> for CalendarWorkflowError {
    fn from(error: AccountReservationError) -> Self {
        match error {
            AccountReservationError::OperationInProgress => Self::OperationInProgress,
            AccountReservationError::StateUnavailable => Self::StateUnavailable,
        }
    }
}

/// A count of provider responses committed by one calendar synchronization request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CalendarSyncReport {
    response_count: usize,
}

impl CalendarSyncReport {
    /// Returns the number of provider responses committed to the cache.
    #[must_use]
    pub fn response_count(self) -> usize {
        self.response_count
    }
}

/// Cached occurrences and cursor state loaded for a fixed view range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarSnapshot {
    events: Vec<CalendarOccurrence>,
    sync_state: CalendarSyncState,
    pending_creation: Option<PendingCalendarEventCreation>,
    write_readiness: CalendarWriteReadiness,
}

impl CalendarSnapshot {
    /// Returns the cached occurrences that overlap the fixed view.
    #[must_use]
    pub fn events(&self) -> &[CalendarOccurrence] {
        &self.events
    }

    /// Returns the saved cursor state for the fixed view.
    #[must_use]
    pub fn sync_state(&self) -> &CalendarSyncState {
        &self.sync_state
    }

    /// Returns the account's unresolved or rejected event-creation operation.
    #[must_use]
    pub fn pending_creation(&self) -> Option<&PendingCalendarEventCreation> {
        self.pending_creation.as_ref()
    }

    /// Returns whether writes are ready for this view or which sync remains required.
    #[must_use]
    pub fn write_readiness(&self) -> CalendarWriteReadiness {
        self.write_readiness
    }
}

/// The result of a Graph event creation and its pending cache state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalendarEventCreationOutcome {
    /// Graph confirmed the event and the encrypted cache committed it.
    Cached(CalendarOccurrence),
    /// Graph confirmed the event, but the encrypted cache still needs recovery.
    ConfirmedButNotCached {
        /// The event returned by Graph.
        event: CalendarOccurrence,
        /// The user-safe category returned by the cache boundary.
        cache_error: CalendarCacheError,
    },
    /// Graph rejected the request and the cache removed its pending operation.
    Rejected,
    /// Graph rejected the request, but the cache still has the pending operation.
    RejectedButPending {
        /// The user-safe category returned by the cache boundary.
        cache_error: CalendarCacheError,
    },
}

/// The Graph update was confirmed and whether the encrypted cache committed it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalendarEventUpdateOutcome {
    /// Graph confirmed the event and the encrypted cache committed it.
    Cached(CalendarOccurrence),
    /// Graph confirmed the event, but the encrypted cache still needs recovery.
    ConfirmedButNotCached {
        /// The event returned by Graph.
        event: CalendarOccurrence,
        /// The user-safe category returned by the cache boundary.
        cache_error: CalendarCacheError,
    },
}

/// The result of a confirmed event deletion and its cache update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarEventDeletionOutcome {
    /// Graph confirmed deletion and the encrypted cache removed every copy.
    Deleted,
    /// Graph confirmed deletion, but the encrypted cache still needs recovery.
    DeletedButNotCached {
        /// The user-safe category returned by the cache boundary.
        cache_error: CalendarCacheError,
    },
}

struct CalendarSyncRound {
    state: CalendarSyncState,
    seen_requests: HashSet<Option<CalendarDeltaLink>>,
    page_count: usize,
    response_bytes: usize,
    reset_count: usize,
}

impl CalendarSyncRound {
    fn new(state: CalendarSyncState) -> Self {
        Self {
            state,
            seen_requests: HashSet::new(),
            page_count: 0,
            response_bytes: 0,
            reset_count: 0,
        }
    }

    fn next_request(&mut self) -> Result<CalendarDeltaRequest, CalendarWorkflowError> {
        let request = self.state.request();
        if !self.seen_requests.insert(request_cursor(&request)) {
            return Err(CalendarWorkflowError::RepeatedCursor);
        }
        Ok(request)
    }

    fn validate_response(
        &mut self,
        page: &CalendarDeltaPage,
        page_bytes: usize,
    ) -> Result<(), CalendarWorkflowError> {
        self.add_response_bytes(page_bytes)?;
        if let einfach_mail_domain::CalendarDeltaProgress::Next(link) = page.progress()
            && self.seen_requests.contains(&Some(link.clone()))
        {
            return Err(CalendarWorkflowError::RepeatedCursor);
        }
        Ok(())
    }

    fn add_response_bytes(&mut self, response_bytes: usize) -> Result<(), CalendarWorkflowError> {
        self.response_bytes = response_bytes_after(self.response_bytes, response_bytes)
            .ok_or(CalendarWorkflowError::ResponseLimitExceeded)?;
        Ok(())
    }

    fn commit(&mut self, state: CalendarSyncState) {
        self.state = state;
        self.page_count += 1;
    }

    fn restart_after_reset(&mut self, state: CalendarSyncState) -> bool {
        self.state = state;
        self.seen_requests.clear();
        self.reset_count += 1;
        self.reset_count <= MAX_DELTA_RESETS_PER_ATTEMPT
    }

    fn is_complete(&self) -> bool {
        self.state.next_link().is_none()
    }

    fn reached_page_limit(&self) -> bool {
        self.page_count == MAX_DELTA_PAGES_PER_ATTEMPT
    }
}

/// Coordinates calendar synchronization, event writes, and encrypted caching.
pub struct CalendarWorkflow<P, C>
where
    P: CalendarProvider,
    C: CalendarCache,
{
    account_id: AccountId,
    provider: P,
    cache: C,
    reservations: OperationReservations,
    recovery: Arc<AccountWorkflowRecovery>,
}

impl<P, C> CalendarWorkflow<P, C>
where
    P: CalendarProvider,
    C: CalendarCache,
{
    /// Creates a workflow bound to one account, provider, and encrypted cache adapter.
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

    /// Loads cached events and cursor state for a fixed view range.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarWorkflowError::AccountMismatch`] for a view from another account, a
    /// cache error when the account or stored view cannot be read, or
    /// [`CalendarWorkflowError::StateUnavailable`] when shared recovery state
    /// cannot be read.
    pub async fn load_cached_view(
        &self,
        view: &CalendarViewReference,
    ) -> Result<CalendarSnapshot, CalendarWorkflowError> {
        self.ensure_account(view.account_id())?;
        let sync_state = self.cache.ensure_view(view).await?;
        let events = self.cache.list_events(view).await?;
        let pending_creation = self.cache.pending_event_creation(view.account_id()).await?;
        let account_has_active_replacement = self
            .cache
            .account_has_active_replacement(view.account_id())
            .await?;
        let account_has_write_sync_required = self
            .cache
            .account_has_calendar_write_sync_required(view.account_id())
            .await?;
        let write_readiness = if account_has_active_replacement || account_has_write_sync_required {
            CalendarWriteReadiness::AccountSyncRequired
        } else {
            self.recovery
                .calendar_write_readiness(view)
                .map_err(|_| CalendarWorkflowError::StateUnavailable)?
        };
        Ok(CalendarSnapshot {
            events,
            sync_state,
            pending_creation,
            write_readiness,
        })
    }

    /// Synchronizes one fixed primary-calendar view using the provider's strategy.
    ///
    /// Delta providers commit each page and its continuation link together. A
    /// snapshot provider replaces the view only after it returns a complete
    /// response and the cache commits it.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarWorkflowError::AccountMismatch`] for a view from another account, a
    /// provider or cache error when a sync response cannot be loaded or
    /// committed, [`CalendarWorkflowError::RepeatedCursor`] for a looped link,
    /// a page, response, or reset limit error while preserving resumable state,
    /// [`CalendarWorkflowError::OperationInProgress`] when another operation
    /// owns the account, or [`CalendarWorkflowError::StateUnavailable`] when
    /// the account reservation or shared recovery state cannot be accessed.
    pub async fn sync_view(
        &self,
        view: &CalendarViewReference,
    ) -> Result<CalendarSyncReport, CalendarWorkflowError> {
        self.ensure_account(view.account_id())?;
        let _reservation = self
            .reservations
            .reserve_calendar_view(view)
            .map_err(CalendarWorkflowError::from)?;
        let strategy = self.provider.calendar_sync_strategy().await?;
        let report = self.sync_by_strategy(view, strategy).await?;
        self.recovery
            .mark_calendar_view_synchronized(view)
            .map_err(|_| CalendarWorkflowError::StateUnavailable)?;
        Ok(report)
    }

    /// Creates one staged event and caches it only after Graph confirms it.
    ///
    /// The exact draft and retry identifier are stored before the network
    /// request. Unknown outcomes keep that operation available for replay.
    ///
    /// A cache failure after Graph confirmation is returned as
    /// [`CalendarEventCreationOutcome::ConfirmedButNotCached`] so callers can
    /// report the remote result accurately. A definitive Graph rejection is
    /// returned as [`CalendarEventCreationOutcome::Rejected`] when cleanup
    /// succeeds, or [`CalendarEventCreationOutcome::RejectedButPending`] when
    /// local cleanup fails.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarWorkflowError::AccountMismatch`] for a creation from another account,
    /// a provider or cache error when staging fails or Graph cannot
    /// confirm the request, [`CalendarWorkflowError::OperationInProgress`]
    /// when another account operation owns the calendar,
    /// [`CalendarWorkflowError::CalendarWriteSyncRequired`] until this view and
    /// all recovery-required account views have completed successful syncs,
    /// [`CalendarWorkflowError::InvalidConfirmation`] when Graph returns an
    /// event that does not match the staged draft, or
    /// [`CalendarWorkflowError::StateUnavailable`] when the account reservation
    /// or shared recovery state cannot be accessed.
    pub async fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> Result<CalendarEventCreationOutcome, CalendarWorkflowError> {
        self.ensure_account(creation.view().account_id())?;
        let _reservation = self
            .reservations
            .reserve_account(creation.view().account_id())
            .map_err(CalendarWorkflowError::from)?;
        self.ensure_calendar_write_ready(creation.view()).await?;
        self.cache.stage_event_creation(creation).await?;
        let event = match self.provider.create_event(creation).await {
            Ok(event) => event,
            Err(CalendarOperationError::EventCreationRejected) => {
                if let Err(cache_error) = self.cache.mark_event_creation_rejected(creation).await {
                    return Ok(CalendarEventCreationOutcome::RejectedButPending { cache_error });
                }
                return match self.cache.discard_event_creation(creation).await {
                    Ok(()) => Ok(CalendarEventCreationOutcome::Rejected),
                    Err(cache_error) => {
                        Ok(CalendarEventCreationOutcome::RejectedButPending { cache_error })
                    }
                };
            }
            Err(error) => return Err(error.into()),
        };
        if !confirms_creation(creation, &event) {
            return Err(CalendarWorkflowError::InvalidConfirmation);
        }
        match self.cache.commit_event_creation(creation, &event).await {
            Ok(()) => Ok(CalendarEventCreationOutcome::Cached(event)),
            Err(cache_error) => {
                Ok(CalendarEventCreationOutcome::ConfirmedButNotCached { event, cache_error })
            }
        }
    }

    /// Removes a pending draft after the provider definitively rejects it.
    ///
    /// Call this with the same operation passed to `create_event` when it
    /// returned [`CalendarEventCreationOutcome::RejectedButPending`]. This
    /// retries local cleanup without another Graph request.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarWorkflowError::AccountMismatch`] for a creation from another account,
    /// a cache error if the matching operation cannot be removed,
    /// [`CalendarWorkflowError::OperationInProgress`] when another calendar
    /// operation owns the view, or
    /// [`CalendarWorkflowError::StateUnavailable`] when the account reservation
    /// cannot be accessed.
    pub async fn discard_rejected_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> Result<(), CalendarWorkflowError> {
        self.ensure_account(creation.view().account_id())?;
        let _reservation = self
            .reservations
            .reserve_calendar_view(creation.view())
            .map_err(CalendarWorkflowError::from)?;
        self.cache.mark_event_creation_rejected(creation).await?;
        self.cache.discard_event_creation(creation).await?;
        Ok(())
    }

    /// Loads the current event after reserving its account against calendar writes.
    ///
    /// The account reservation ends when this method returns so the user can
    /// edit the form without blocking calendar synchronization.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarWorkflowError::AccountMismatch`] for an event from another account,
    /// a provider error when the event cannot be read or edited, a
    /// [`CalendarWorkflowError::OperationInProgress`] when another calendar
    /// operation owns the account, or
    /// [`CalendarWorkflowError::CalendarWriteSyncRequired`] when this view has
    /// not completed a successful sync or another view for the account needs
    /// recovery after a calendar write. Returns
    /// [`CalendarWorkflowError::InvalidEditableEvent`]
    /// when the provider returns a snapshot for another event. Returns
    /// [`CalendarWorkflowError::StateUnavailable`] when the account reservation
    /// or shared recovery state cannot be accessed.
    pub async fn editable_event(
        &self,
        event: &CalendarOccurrence,
    ) -> Result<EditableCalendarEvent, CalendarWorkflowError> {
        let account_id = event.reference().account_id();
        self.ensure_account(account_id)?;
        let _reservation = self
            .reservations
            .reserve_account(account_id)
            .map_err(CalendarWorkflowError::from)?;
        self.ensure_calendar_write_ready(event.reference()).await?;
        let editable = self.provider.editable_event(event).await?;
        if editable.occurrence().reference() != event.reference()
            || editable.occurrence().id() != event.id()
        {
            return Err(CalendarWorkflowError::InvalidEditableEvent);
        }
        Ok(editable)
    }

    /// Updates one event and atomically reconciles every cached account view.
    ///
    /// The account reservation covers the provider write and cache transaction.
    /// A changed event, uncertain provider result, or confirmed update that
    /// cannot be cached requires successful syncs for previously synchronized
    /// account views before another calendar write can start. Dropping this
    /// future after the durable recovery markers are written also requires
    /// those syncs.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarWorkflowError::AccountMismatch`] for an update from another account,
    /// a provider error for a rejected, stale, or uncertain update,
    /// [`CalendarWorkflowError::OperationInProgress`] when another calendar
    /// operation owns the account, or
    /// [`CalendarWorkflowError::CalendarWriteSyncRequired`] when this view has
    /// not completed a successful sync or another view for the account needs
    /// recovery, or
    /// [`CalendarWorkflowError::StateUnavailable`] when the account reservation
    /// or shared recovery state cannot be accessed,
    /// [`CalendarWorkflowError::InvalidUpdateConfirmation`] when the provider
    /// returns different event values. A cache failure after Graph confirmation
    /// is returned as [`CalendarEventUpdateOutcome::ConfirmedButNotCached`].
    pub async fn update_event(
        &self,
        update: &CalendarEventUpdate,
    ) -> Result<CalendarEventUpdateOutcome, CalendarWorkflowError> {
        let view = update.source().occurrence().reference();
        self.ensure_account(view.account_id())?;
        let account_id = view.account_id().clone();
        let _reservation = self
            .reservations
            .reserve_account(&account_id)
            .map_err(CalendarWorkflowError::from)?;
        self.ensure_calendar_write_ready(view).await?;
        self.cache.mark_calendar_write_sync_required(view).await?;
        let event = match self.provider.update_event(update).await {
            Ok(event) => event,
            Err(error) => {
                if !matches!(
                    error,
                    CalendarOperationError::EventChanged
                        | CalendarOperationError::UpdateOutcomeUnknown
                ) {
                    self.cache
                        .clear_calendar_write_sync_required(&account_id)
                        .await?;
                }
                return Err(error.into());
            }
        };
        if !update.confirms(&event) {
            return Err(CalendarWorkflowError::InvalidUpdateConfirmation);
        }
        match self.cache.commit_event_update(update, &event).await {
            Ok(()) => Ok(CalendarEventUpdateOutcome::Cached(event)),
            Err(cache_error) => {
                Ok(CalendarEventUpdateOutcome::ConfirmedButNotCached { event, cache_error })
            }
        }
    }

    /// Deletes one checked event and removes every cached account copy atomically.
    ///
    /// The account reservation covers the final provider check, Graph request,
    /// and cache transaction. A changed event, uncertain result, or cache
    /// failure requires successful syncs for previously synchronized account
    /// views before another calendar write can start. Dropping this future
    /// after the durable recovery markers are written also requires those
    /// syncs.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarWorkflowError::AccountMismatch`] for an event from another account,
    /// a provider error for a rejected, stale, or uncertain deletion,
    /// [`CalendarWorkflowError::OperationInProgress`] when another operation
    /// owns the account, or [`CalendarWorkflowError::CalendarWriteSyncRequired`]
    /// when the source view has not completed a sync or another account view
    /// needs recovery, or [`CalendarWorkflowError::StateUnavailable`] when the
    /// account reservation or shared recovery state cannot be accessed. A cache
    /// failure after Graph confirms deletion is returned as
    /// [`CalendarEventDeletionOutcome::DeletedButNotCached`].
    pub async fn delete_event(
        &self,
        event: &EditableCalendarEvent,
    ) -> Result<CalendarEventDeletionOutcome, CalendarWorkflowError> {
        let view = event.occurrence().reference();
        self.ensure_account(view.account_id())?;
        let account_id = view.account_id().clone();
        let _reservation = self
            .reservations
            .reserve_account(&account_id)
            .map_err(CalendarWorkflowError::from)?;
        self.ensure_calendar_write_ready(view).await?;
        self.cache.mark_calendar_write_sync_required(view).await?;
        if let Err(error) = self.provider.delete_event(event).await {
            if !matches!(
                error,
                CalendarOperationError::EventChanged | CalendarOperationError::DeleteOutcomeUnknown
            ) {
                self.cache
                    .clear_calendar_write_sync_required(&account_id)
                    .await?;
            }
            return Err(error.into());
        }
        match self.cache.commit_event_deletion(event).await {
            Ok(()) => Ok(CalendarEventDeletionOutcome::Deleted),
            Err(cache_error) => {
                Ok(CalendarEventDeletionOutcome::DeletedButNotCached { cache_error })
            }
        }
    }

    fn ensure_account(&self, account_id: &AccountId) -> Result<(), CalendarWorkflowError> {
        if account_id == &self.account_id {
            Ok(())
        } else {
            Err(CalendarWorkflowError::AccountMismatch)
        }
    }

    async fn ensure_calendar_write_ready(
        &self,
        view: &CalendarViewReference,
    ) -> Result<(), CalendarWorkflowError> {
        let write_sync_required = self
            .cache
            .account_has_active_replacement(view.account_id())
            .await?
            || self
                .cache
                .account_has_calendar_write_sync_required(view.account_id())
                .await?;
        if write_sync_required {
            return Err(CalendarWorkflowError::CalendarWriteSyncRequired);
        }
        match self
            .recovery
            .calendar_write_readiness(view)
            .map_err(|_| CalendarWorkflowError::StateUnavailable)?
        {
            CalendarWriteReadiness::Ready => Ok(()),
            CalendarWriteReadiness::ViewSyncRequired
            | CalendarWriteReadiness::AccountSyncRequired => {
                Err(CalendarWorkflowError::CalendarWriteSyncRequired)
            }
        }
    }

    async fn sync_by_strategy(
        &self,
        view: &CalendarViewReference,
        strategy: CalendarSyncStrategy,
    ) -> Result<CalendarSyncReport, CalendarWorkflowError> {
        match strategy {
            CalendarSyncStrategy::Delta => {
                let state = self.cache.ensure_view(view).await?;
                self.sync_round(view, state).await
            }
            CalendarSyncStrategy::Snapshot => self.sync_snapshot(view).await,
        }
    }

    async fn sync_snapshot(
        &self,
        view: &CalendarViewReference,
    ) -> Result<CalendarSyncReport, CalendarWorkflowError> {
        let response = self.provider.calendar_view_snapshot(view).await?;
        let (events, response_bytes) = response.into_parts();
        if response_bytes_after(0, response_bytes).is_none() {
            return Err(CalendarWorkflowError::ResponseLimitExceeded);
        }
        self.cache
            .replace_snapshot(view, events, SystemTime::now())
            .await?;
        Ok(CalendarSyncReport { response_count: 1 })
    }

    async fn sync_round(
        &self,
        view: &CalendarViewReference,
        state: CalendarSyncState,
    ) -> Result<CalendarSyncReport, CalendarWorkflowError> {
        let mut round = CalendarSyncRound::new(state);
        loop {
            if self.sync_next_page(view, &mut round).await? {
                return Ok(CalendarSyncReport {
                    response_count: round.page_count,
                });
            }
            if round.reached_page_limit() {
                return Err(CalendarWorkflowError::PageLimitExceeded);
            }
        }
    }

    async fn sync_next_page(
        &self,
        view: &CalendarViewReference,
        round: &mut CalendarSyncRound,
    ) -> Result<bool, CalendarWorkflowError> {
        let request = round.next_request()?;
        match self
            .provider
            .calendar_delta_page(view, request.clone())
            .await?
        {
            CalendarDeltaResponse::Page(response) => {
                let (page, page_bytes) = response.into_parts();
                round.validate_response(&page, page_bytes)?;
                let state = self
                    .cache
                    .commit_delta_page(view, page, SystemTime::now())
                    .await?;
                round.commit(state);
                Ok(round.is_complete())
            }
            CalendarDeltaResponse::Reset {
                reset_link,
                response_bytes,
            } => {
                if request == CalendarDeltaRequest::Initial {
                    return Err(CalendarOperationError::InvalidResponse.into());
                }
                round.add_response_bytes(response_bytes)?;
                let state = self.cache.begin_full_resync(view, reset_link).await?;
                if !round.restart_after_reset(state) {
                    return Err(CalendarWorkflowError::DeltaResetLimitExceeded);
                }
                Ok(false)
            }
        }
    }
}

fn confirms_creation(creation: &CalendarEventCreation, event: &CalendarOccurrence) -> bool {
    event.reference() == creation.view()
        && event.subject() == Some(creation.draft().subject())
        && event.time() == creation.draft().time()
        && event.overlaps_view()
}

fn request_cursor(request: &CalendarDeltaRequest) -> Option<CalendarDeltaLink> {
    match request {
        CalendarDeltaRequest::Initial => None,
        CalendarDeltaRequest::Continue(link) => Some(link.clone()),
    }
}

#[cfg(test)]
mod tests;
