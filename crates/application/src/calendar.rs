//! Calendar provider and cache contracts used by account-scoped workflows.

use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use einfach_mail_domain::{
    AccountId, CalendarDeltaLink, CalendarDeltaPage, CalendarEventCreation, CalendarEventUpdate,
    CalendarOccurrence, CalendarViewReference, EditableCalendarEvent, PendingCalendarEventCreation,
};
use thiserror::Error;

use crate::{
    AccountOperationError,
    mail_workflow::{
        AccountReservationError, ActiveOperations, reserve_account, reserve_calendar_view,
    },
};

/// A user-safe category for a calendar provider failure.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CalendarOperationError {
    /// The requested account has no active provider session.
    #[error("sign-in is required")]
    SignInRequired,
    /// The requested view belongs to another connected account.
    #[error("the calendar view belongs to another account")]
    AccountMismatch,
    /// The provider denied access to the calendar operation.
    #[error("calendar access was denied")]
    AccessDenied,
    /// The provider confirmed that the event request was rejected.
    #[error("the calendar event request was rejected")]
    EventCreationRejected,
    /// The event may exist, but the provider could not confirm its creation.
    #[error("the calendar event creation outcome is unknown")]
    CreationOutcomeUnknown,
    /// The service asked the application to wait before retrying.
    #[error("the calendar service is throttling requests")]
    RateLimited {
        /// The advised delay when the provider returned one.
        retry_after: Option<std::time::Duration>,
    },
    /// The provider could not complete the network request.
    #[error("the calendar service is unavailable")]
    ServiceUnavailable,
    /// The provider returned data that cannot be mapped to calendar values.
    #[error("the calendar service returned invalid data")]
    InvalidResponse,
    /// The response or traversal exceeded provider resource limits.
    #[error("the calendar request exceeded its resource limits")]
    ResourceLimitExceeded,
    /// The current event does not satisfy the supported write rules.
    #[error("this calendar event cannot be changed by Einfach Mail")]
    EventNotEditable,
    /// The event changed or became unsupported while the editor was open.
    #[error("this calendar event changed; refresh it before changing it again")]
    EventChanged,
    /// Graph may have applied the update, but the client did not receive confirmation.
    #[error("the calendar update outcome is unknown")]
    UpdateOutcomeUnknown,
    /// Graph rejected the confirmed event deletion request.
    #[error("the calendar event deletion was rejected")]
    EventDeletionRejected,
    /// Graph may have deleted the event, but the client did not receive confirmation.
    #[error("the calendar deletion outcome is unknown")]
    DeleteOutcomeUnknown,
}

impl From<AccountOperationError> for CalendarOperationError {
    fn from(error: AccountOperationError) -> Self {
        match error {
            AccountOperationError::AccessDenied => Self::AccessDenied,
            AccountOperationError::AccessTokenRejected | AccountOperationError::SignInExpired => {
                Self::SignInRequired
            }
            _ => Self::ServiceUnavailable,
        }
    }
}

/// Selects the initial view query or the exact opaque link to resume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalendarDeltaRequest {
    /// Starts the view's initial delta round.
    Initial,
    /// Resumes the exact next-page or completed delta link stored for this view.
    Continue(CalendarDeltaLink),
}

/// One provider delta page and the number of response-body bytes received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarDeltaPageResponse {
    page: CalendarDeltaPage,
    response_bytes: usize,
}

impl CalendarDeltaPageResponse {
    /// Creates a response with its parsed page and received body size.
    #[must_use]
    pub fn new(page: CalendarDeltaPage, response_bytes: usize) -> Self {
        Self {
            page,
            response_bytes,
        }
    }

    /// Returns the response body's byte length.
    #[must_use]
    pub fn response_bytes(&self) -> usize {
        self.response_bytes
    }

    /// Returns the parsed page and response size.
    #[must_use]
    pub fn into_parts(self) -> (CalendarDeltaPage, usize) {
        (self.page, self.response_bytes)
    }
}

/// Calendar operations required by the application.
pub trait CalendarProvider: Send + Sync {
    /// Loads one delta page for an account's primary-calendar view.
    ///
    /// The provider rejects a view scoped to another account. Continuation
    /// links are opaque and are validated before the user's token is attached.
    ///
    /// # Errors
    ///
    /// Returns a user-safe provider error when sign-in, access, transport,
    /// response validation, or resource limits prevent loading the page.
    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaPageResponse, CalendarOperationError>> + Send + '_;

    /// Creates one event for the operation's account and exact primary-calendar view.
    ///
    /// The provider uses the operation's stable identifier as the Graph retry
    /// key and never sends attendees or recurrence data.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarOperationError::EventCreationRejected`] only when a
    /// provider response proves Graph did not create the event. Returns
    /// [`CalendarOperationError::CreationOutcomeUnknown`] when the request may
    /// have reached Graph without confirmation. Other errors describe sign-in,
    /// access, transport, or response validation failures.
    fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_;

    /// Loads and validates an event for the supported edit flow.
    ///
    /// The provider must reject events outside the accepted editable shape and
    /// return the event version with its complete current occurrence values.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarOperationError::EventNotEditable`] for an unsupported
    /// event, or a user-safe provider error when the event cannot be read or
    /// its response cannot be validated.
    fn editable_event(
        &self,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<EditableCalendarEvent, CalendarOperationError>> + Send + '_;

    /// Rechecks the captured event version and updates its requested fields.
    ///
    /// Implementations must not automatically retry the Graph write.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarOperationError::EventChanged`] when the version is
    /// stale, [`CalendarOperationError::UpdateOutcomeUnknown`] when the write
    /// may have reached Graph without a verifiable response, or another
    /// user-safe provider error when Graph rejects the request.
    fn update_event(
        &self,
        update: &CalendarEventUpdate,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_;

    /// Rechecks a supported event and deletes it once without automatic retries.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarOperationError::EventChanged`] when the event version
    /// or supported shape changed, [`CalendarOperationError::DeleteOutcomeUnknown`]
    /// when Graph may have deleted it without a verifiable response, or another
    /// user-safe provider error when Graph rejects the request.
    fn delete_event(
        &self,
        event: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_;
}

/// A user-safe category for a local calendar cache failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CalendarCacheError {
    /// The encrypted cache is unavailable.
    #[error("the local calendar cache is unavailable")]
    Unavailable,
    /// Another cache handle or lifecycle operation owns an account lock.
    #[error("the local calendar cache is in use")]
    InUse,
    /// Stored values do not satisfy the calendar domain contract.
    #[error("the local calendar cache contains invalid data")]
    InvalidData,
    /// The requested view belongs to another cache account.
    #[error("the local calendar cache belongs to another account")]
    AccountMismatch,
    /// An unresolved send marker prevents removal of the shared account cache.
    #[error("a pending mail send must be resolved before removing the account cache")]
    PendingMailSend,
    /// Another uncertain event creation must be resolved for this account first.
    #[error("another calendar event creation is already pending")]
    CreationAlreadyPending,
    /// Removing the account cache would discard an unresolved calendar event creation.
    #[error("a calendar event creation needs recovery before account removal")]
    UnresolvedEventCreation,
}

/// An account and view's completed delta cursor and resumable page checkpoint.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarSyncState {
    delta_link: Option<CalendarDeltaLink>,
    next_link: Option<CalendarDeltaLink>,
    last_synchronized_at: Option<SystemTime>,
}

impl CalendarSyncState {
    /// Creates state for a view whose initial delta request has not started.
    #[must_use]
    pub fn initial() -> Self {
        Self {
            delta_link: None,
            next_link: None,
            last_synchronized_at: None,
        }
    }

    /// Restores a checkpoint after persistence validates its stored timestamps.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::InvalidData`] when a completion time
    /// exists without a completed delta link.
    pub fn from_checkpoint(
        delta_link: Option<CalendarDeltaLink>,
        next_link: Option<CalendarDeltaLink>,
        last_synchronized_at: Option<SystemTime>,
    ) -> Result<Self, CalendarCacheError> {
        if last_synchronized_at.is_some() && delta_link.is_none() {
            return Err(CalendarCacheError::InvalidData);
        }
        Ok(Self {
            delta_link,
            next_link,
            last_synchronized_at,
        })
    }

    /// Returns the link for the last completed round, if one exists.
    #[must_use]
    pub fn delta_link(&self) -> Option<&CalendarDeltaLink> {
        self.delta_link.as_ref()
    }

    /// Returns the next-page link for an interrupted round, if one exists.
    #[must_use]
    pub fn next_link(&self) -> Option<&CalendarDeltaLink> {
        self.next_link.as_ref()
    }

    /// Returns when the last full view round committed.
    #[must_use]
    pub fn last_synchronized_at(&self) -> Option<SystemTime> {
        self.last_synchronized_at
    }

    /// Selects the interrupted page before the completed-round cursor.
    #[must_use]
    pub fn request(&self) -> CalendarDeltaRequest {
        self.next_link
            .as_ref()
            .or(self.delta_link.as_ref())
            .map_or(CalendarDeltaRequest::Initial, |link| {
                CalendarDeltaRequest::Continue(link.clone())
            })
    }
}

impl std::fmt::Debug for CalendarSyncState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CalendarSyncState")
            .field(
                "delta_link",
                &self.delta_link.as_ref().map(|_| "<redacted>"),
            )
            .field("next_link", &self.next_link.as_ref().map(|_| "<redacted>"))
            .field("last_synchronized_at", &self.last_synchronized_at)
            .finish()
    }
}

/// Encrypted local operations for one account's fixed calendar view.
pub trait CalendarCache: Send + Sync {
    /// Returns the view's saved cursor state, creating an empty state when needed.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account,
    /// [`CalendarCacheError::InvalidData`] for malformed stored values, or
    /// [`CalendarCacheError::Unavailable`] when the database cannot be read.
    fn ensure_view(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_;

    /// Applies one event page and advances its checkpoint in the same transaction.
    ///
    /// Incomplete updates apply only to cached occurrences. The cache ignores
    /// upserts that no longer overlap the fixed view and deletes their prior row.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account,
    /// [`CalendarCacheError::InvalidData`] for malformed records, or
    /// [`CalendarCacheError::Unavailable`] when the transaction fails.
    fn commit_delta_page(
        &self,
        view: &CalendarViewReference,
        page: CalendarDeltaPage,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_;

    /// Returns cached events that overlap the requested view range.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account,
    /// [`CalendarCacheError::InvalidData`] for malformed stored events, or
    /// [`CalendarCacheError::Unavailable`] when the cache cannot be read.
    fn list_events(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<Vec<CalendarOccurrence>, CalendarCacheError>> + Send + '_;

    /// Saves or confirms the account's unresolved event-creation operation.
    ///
    /// Repeating the same unresolved operation is idempotent. A rejected
    /// operation remains blocked until its marker is removed.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::CreationAlreadyPending`] while an
    /// unresolved operation or a different operation is stored, an account or
    /// data error for invalid values, or [`CalendarCacheError::Unavailable`]
    /// when the encrypted store cannot be changed.
    fn stage_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;

    /// Returns the account's pending event-creation operation, if present.
    ///
    /// # Errors
    ///
    /// Returns an account or stored-data error when the record cannot be
    /// validated, or [`CalendarCacheError::Unavailable`] when the store cannot
    /// be read.
    fn pending_event_creation(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Option<PendingCalendarEventCreation>, CalendarCacheError>> + Send + '_;

    /// Records that Graph definitively rejected the matching operation.
    ///
    /// Repeating this update is safe. An absent matching marker is already
    /// resolved, while another operation remains untouched.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::CreationAlreadyPending`] for another
    /// operation, an account or data error for invalid input, or
    /// [`CalendarCacheError::Unavailable`] when the encrypted store cannot
    /// record the rejection.
    fn mark_event_creation_rejected(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;

    /// Removes the exact rejected operation after the provider definitively rejects it.
    ///
    /// Removing an operation that is already absent is safe. An unresolved
    /// operation or a different pending operation remains untouched.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::CreationAlreadyPending`] when another
    /// operation is stored, an account or data error for invalid values, or
    /// [`CalendarCacheError::Unavailable`] when the encrypted store cannot be
    /// changed.
    fn discard_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;

    /// Stores a Graph-confirmed event and removes its matching pending operation atomically.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::InvalidData`] when the event does not
    /// confirm the staged draft, an account error for a mismatched view, or
    /// [`CalendarCacheError::Unavailable`] when the transaction fails.
    fn commit_event_creation(
        &self,
        creation: &CalendarEventCreation,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;

    /// Replaces every cached copy of a Graph-confirmed event in one transaction.
    ///
    /// The cache removes the old event from all ranges for its account, then
    /// writes it to every existing range that the updated occurrence overlaps.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::InvalidData`] when the event does not
    /// confirm the requested update, [`CalendarCacheError::AccountMismatch`]
    /// for a different cache account, or [`CalendarCacheError::Unavailable`]
    /// when the transaction fails.
    fn commit_event_update(
        &self,
        update: &CalendarEventUpdate,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;

    /// Removes every cached copy of the provider-confirmed event atomically.
    ///
    /// The source view must still exist. Calendar-view cursors remain unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account or
    /// [`CalendarCacheError::Unavailable`] when the transaction cannot commit.
    fn commit_event_deletion(
        &self,
        event: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;

    /// Removes all account data owned by this cache and its local encryption key.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account,
    /// [`CalendarCacheError::PendingMailSend`] while a send outcome is unresolved,
    /// [`CalendarCacheError::UnresolvedEventCreation`] while an event-creation outcome is
    /// unresolved, [`CalendarCacheError::InUse`] while another cache handle or lifecycle
    /// operation owns an account lock, or
    /// [`CalendarCacheError::Unavailable`] when cache removal fails.
    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;
}

/// A user-safe category for a calendar workflow failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CalendarWorkflowError {
    /// The provider could not load a delta page.
    #[error("the calendar service is unavailable")]
    Provider(#[from] CalendarOperationError),
    /// The encrypted local cache could not read or commit events.
    #[error("the local calendar cache is unavailable")]
    Cache(#[from] CalendarCacheError),
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
    /// A complete calendar sync is required before an event write.
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
    /// The synchronization reservation could not be read or released.
    #[error("the calendar workflow state is unavailable")]
    StateUnavailable,
}

/// A count of pages committed by one calendar synchronization request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CalendarSyncReport {
    page_count: usize,
}

impl CalendarSyncReport {
    /// Returns the number of delta pages committed.
    #[must_use]
    pub fn page_count(self) -> usize {
        self.page_count
    }
}

/// Calendar write state for one view and its account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarWriteReadiness {
    /// This view and all recovery-required views for the account are synchronized.
    Ready,
    /// This view has not completed a successful sync in the current workflow.
    ViewSyncRequired,
    /// At least one previously synchronized view for this account needs recovery.
    AccountSyncRequired,
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

const MAX_PAGES_PER_VIEW_ROUND: usize = 1_000;
const MAX_SYNC_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

struct CalendarSyncRound {
    state: CalendarSyncState,
    seen_requests: std::collections::HashSet<Option<CalendarDeltaLink>>,
    page_count: usize,
    response_bytes: usize,
}

impl CalendarSyncRound {
    fn new(state: CalendarSyncState) -> Self {
        Self {
            state,
            seen_requests: std::collections::HashSet::new(),
            page_count: 0,
            response_bytes: 0,
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
        self.response_bytes = self
            .response_bytes
            .checked_add(page_bytes)
            .filter(|total| *total <= MAX_SYNC_RESPONSE_BYTES)
            .ok_or(CalendarWorkflowError::ResponseLimitExceeded)?;
        if let einfach_mail_domain::CalendarDeltaProgress::Next(link) = page.progress()
            && self.seen_requests.contains(&Some(link.clone()))
        {
            return Err(CalendarWorkflowError::RepeatedCursor);
        }
        Ok(())
    }

    fn commit(&mut self, state: CalendarSyncState) {
        self.state = state;
        self.page_count += 1;
    }

    fn is_complete(&self) -> bool {
        self.state.next_link().is_none()
    }

    fn reached_page_limit(&self) -> bool {
        self.page_count == MAX_PAGES_PER_VIEW_ROUND
    }
}

/// Coordinates calendar synchronization, event writes, and encrypted caching.
pub struct CalendarWorkflow<P, C>
where
    P: CalendarProvider,
    C: CalendarCache,
{
    provider: P,
    cache: C,
    reservations: Arc<Mutex<ActiveOperations>>,
    write_recovery: Mutex<CalendarWriteRecovery>,
}

#[derive(Default)]
struct CalendarWriteRecovery {
    synchronized_views: std::collections::HashSet<CalendarViewReference>,
    sync_required_views: std::collections::HashSet<CalendarViewReference>,
}

impl CalendarWriteRecovery {
    fn readiness_for(&self, view: &CalendarViewReference) -> CalendarWriteReadiness {
        if self
            .sync_required_views
            .iter()
            .any(|required_view| required_view.account_id() == view.account_id())
        {
            CalendarWriteReadiness::AccountSyncRequired
        } else if self.synchronized_views.contains(view) {
            CalendarWriteReadiness::Ready
        } else {
            CalendarWriteReadiness::ViewSyncRequired
        }
    }
}

impl<P, C> CalendarWorkflow<P, C>
where
    P: CalendarProvider,
    C: CalendarCache,
{
    /// Creates a workflow from one provider and encrypted cache adapter.
    #[must_use]
    pub fn new(provider: P, cache: C) -> Self {
        Self::with_reservations(
            provider,
            cache,
            Arc::new(Mutex::new(ActiveOperations::default())),
        )
    }

    pub(crate) fn with_reservations(
        provider: P,
        cache: C,
        reservations: Arc<Mutex<ActiveOperations>>,
    ) -> Self {
        Self {
            provider,
            cache,
            reservations,
            write_recovery: Mutex::new(CalendarWriteRecovery::default()),
        }
    }

    /// Loads cached events and cursor state for a fixed view range.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account or stored view cannot be read.
    pub async fn load_cached_view(
        &self,
        view: &CalendarViewReference,
    ) -> Result<CalendarSnapshot, CalendarWorkflowError> {
        let sync_state = self.cache.ensure_view(view).await?;
        let events = self.cache.list_events(view).await?;
        let pending_creation = self.cache.pending_event_creation(view.account_id()).await?;
        let recovery = self
            .write_recovery
            .lock()
            .map_err(|_| CalendarWorkflowError::StateUnavailable)?;
        let write_readiness = recovery.readiness_for(view);
        Ok(CalendarSnapshot {
            events,
            sync_state,
            pending_creation,
            write_readiness,
        })
    }

    /// Synchronizes all pages for one fixed primary-calendar view.
    ///
    /// Each page and its continuation link commit together. An interrupted
    /// round resumes at its saved next link the next time this method runs.
    ///
    /// # Errors
    ///
    /// Returns a provider or cache error when a page cannot be loaded or
    /// committed, [`CalendarWorkflowError::RepeatedCursor`] for a looped link,
    /// a page or response limit error while preserving resumable state, or
    /// [`CalendarWorkflowError::OperationInProgress`] when another operation
    /// owns the account.
    pub async fn sync_view(
        &self,
        view: &CalendarViewReference,
    ) -> Result<CalendarSyncReport, CalendarWorkflowError> {
        let _reservation =
            reserve_calendar_view(&self.reservations, view).map_err(|error| match error {
                AccountReservationError::OperationInProgress => {
                    CalendarWorkflowError::OperationInProgress
                }
                AccountReservationError::StateUnavailable => {
                    CalendarWorkflowError::StateUnavailable
                }
            })?;
        let state = self.cache.ensure_view(view).await?;
        let report = self.sync_round(view, state).await?;
        self.mark_view_synchronized(view)?;
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
    /// Returns a provider or cache error when staging fails or Graph cannot
    /// confirm the request, [`CalendarWorkflowError::OperationInProgress`]
    /// when another account operation owns the calendar,
    /// [`CalendarWorkflowError::CalendarWriteSyncRequired`] until this view and
    /// all recovery-required account views have completed successful syncs,
    /// [`CalendarWorkflowError::InvalidConfirmation`] when Graph returns an
    /// event that does not match the staged draft, or
    /// [`CalendarWorkflowError::StateUnavailable`] when workflow state cannot
    /// be read.
    pub async fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> Result<CalendarEventCreationOutcome, CalendarWorkflowError> {
        let _reservation = reserve_calendar_view(&self.reservations, creation.view()).map_err(
            |error| match error {
                AccountReservationError::OperationInProgress => {
                    CalendarWorkflowError::OperationInProgress
                }
                AccountReservationError::StateUnavailable => {
                    CalendarWorkflowError::StateUnavailable
                }
            },
        )?;
        self.ensure_calendar_write_ready(creation.view())?;
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
    /// Returns a cache error if the matching operation cannot be removed, or a
    /// workflow error when another calendar operation owns the view.
    pub async fn discard_rejected_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> Result<(), CalendarWorkflowError> {
        let _reservation = reserve_calendar_view(&self.reservations, creation.view()).map_err(
            |error| match error {
                AccountReservationError::OperationInProgress => {
                    CalendarWorkflowError::OperationInProgress
                }
                AccountReservationError::StateUnavailable => {
                    CalendarWorkflowError::StateUnavailable
                }
            },
        )?;
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
    /// Returns a provider error when the event cannot be read or edited, a
    /// [`CalendarWorkflowError::OperationInProgress`] when another calendar
    /// operation owns the account, or
    /// [`CalendarWorkflowError::CalendarWriteSyncRequired`] when this view has
    /// not completed a successful sync or another view for the account needs
    /// recovery after a calendar write. Returns
    /// [`CalendarWorkflowError::InvalidEditableEvent`]
    /// when the provider returns a snapshot for another event. Returns
    /// [`CalendarWorkflowError::StateUnavailable`] when workflow state is poisoned.
    pub async fn editable_event(
        &self,
        event: &CalendarOccurrence,
    ) -> Result<EditableCalendarEvent, CalendarWorkflowError> {
        let account_id = event.reference().account_id();
        let _reservation =
            reserve_account(&self.reservations, account_id).map_err(|error| match error {
                AccountReservationError::OperationInProgress => {
                    CalendarWorkflowError::OperationInProgress
                }
                AccountReservationError::StateUnavailable => {
                    CalendarWorkflowError::StateUnavailable
                }
            })?;
        self.ensure_calendar_write_ready(event.reference())?;
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
    /// account views before another calendar write can start.
    ///
    /// # Errors
    ///
    /// Returns a provider error for a rejected, stale, or uncertain update,
    /// [`CalendarWorkflowError::OperationInProgress`] when another calendar
    /// operation owns the account, or
    /// [`CalendarWorkflowError::CalendarWriteSyncRequired`] when this view has
    /// not completed a successful sync or another view for the account needs
    /// recovery, or
    /// [`CalendarWorkflowError::StateUnavailable`] when workflow state is poisoned. Returns
    /// [`CalendarWorkflowError::InvalidUpdateConfirmation`] when the provider
    /// returns different event values. A cache failure after Graph confirmation
    /// is returned as [`CalendarEventUpdateOutcome::ConfirmedButNotCached`].
    pub async fn update_event(
        &self,
        update: &CalendarEventUpdate,
    ) -> Result<CalendarEventUpdateOutcome, CalendarWorkflowError> {
        let view = update.source().occurrence().reference();
        let account_id = view.account_id();
        let _reservation =
            reserve_account(&self.reservations, account_id).map_err(|error| match error {
                AccountReservationError::OperationInProgress => {
                    CalendarWorkflowError::OperationInProgress
                }
                AccountReservationError::StateUnavailable => {
                    CalendarWorkflowError::StateUnavailable
                }
            })?;
        self.ensure_calendar_write_ready(view)?;
        let event = match self.provider.update_event(update).await {
            Ok(event) => event,
            Err(error) => {
                if matches!(
                    error,
                    CalendarOperationError::EventChanged
                        | CalendarOperationError::UpdateOutcomeUnknown
                ) {
                    self.require_account_calendar_write_sync(account_id)?;
                }
                return Err(error.into());
            }
        };
        if !update.confirms(&event) {
            self.require_account_calendar_write_sync(account_id)?;
            return Err(CalendarWorkflowError::InvalidUpdateConfirmation);
        }
        match self.cache.commit_event_update(update, &event).await {
            Ok(()) => Ok(CalendarEventUpdateOutcome::Cached(event)),
            Err(cache_error) => {
                self.require_account_calendar_write_sync(account_id)?;
                Ok(CalendarEventUpdateOutcome::ConfirmedButNotCached { event, cache_error })
            }
        }
    }

    /// Deletes one checked event and removes every cached account copy atomically.
    ///
    /// The account reservation covers the final provider check, Graph request,
    /// and cache transaction. A changed event, uncertain result, or cache
    /// failure requires successful syncs for previously synchronized account
    /// views before another calendar write can start.
    ///
    /// # Errors
    ///
    /// Returns a provider error for a rejected, stale, or uncertain deletion,
    /// [`CalendarWorkflowError::OperationInProgress`] when another operation
    /// owns the account, or [`CalendarWorkflowError::CalendarWriteSyncRequired`]
    /// when the source view has not completed a sync or another account view
    /// needs recovery. A cache failure after Graph confirms deletion is returned as
    /// [`CalendarEventDeletionOutcome::DeletedButNotCached`].
    pub async fn delete_event(
        &self,
        event: &EditableCalendarEvent,
    ) -> Result<CalendarEventDeletionOutcome, CalendarWorkflowError> {
        let view = event.occurrence().reference();
        let account_id = view.account_id();
        let _reservation =
            reserve_account(&self.reservations, account_id).map_err(|error| match error {
                AccountReservationError::OperationInProgress => {
                    CalendarWorkflowError::OperationInProgress
                }
                AccountReservationError::StateUnavailable => {
                    CalendarWorkflowError::StateUnavailable
                }
            })?;
        self.ensure_calendar_write_ready(view)?;
        if let Err(error) = self.provider.delete_event(event).await {
            if matches!(
                error,
                CalendarOperationError::EventChanged | CalendarOperationError::DeleteOutcomeUnknown
            ) {
                self.require_account_calendar_write_sync(account_id)?;
            }
            return Err(error.into());
        }
        match self.cache.commit_event_deletion(event).await {
            Ok(()) => Ok(CalendarEventDeletionOutcome::Deleted),
            Err(cache_error) => {
                self.require_account_calendar_write_sync(account_id)?;
                Ok(CalendarEventDeletionOutcome::DeletedButNotCached { cache_error })
            }
        }
    }

    fn ensure_calendar_write_ready(
        &self,
        view: &CalendarViewReference,
    ) -> Result<(), CalendarWorkflowError> {
        let recovery = self
            .write_recovery
            .lock()
            .map_err(|_| CalendarWorkflowError::StateUnavailable)?;
        match recovery.readiness_for(view) {
            CalendarWriteReadiness::Ready => Ok(()),
            CalendarWriteReadiness::ViewSyncRequired
            | CalendarWriteReadiness::AccountSyncRequired => {
                Err(CalendarWorkflowError::CalendarWriteSyncRequired)
            }
        }
    }

    fn require_account_calendar_write_sync(
        &self,
        account_id: &AccountId,
    ) -> Result<(), CalendarWorkflowError> {
        let mut recovery = self
            .write_recovery
            .lock()
            .map_err(|_| CalendarWorkflowError::StateUnavailable)?;
        let synchronized_account_views = recovery
            .synchronized_views
            .iter()
            .filter(|view| view.account_id() == account_id)
            .cloned()
            .collect::<Vec<_>>();
        recovery
            .sync_required_views
            .extend(synchronized_account_views);
        Ok(())
    }

    fn mark_view_synchronized(
        &self,
        view: &CalendarViewReference,
    ) -> Result<(), CalendarWorkflowError> {
        let mut recovery = self
            .write_recovery
            .lock()
            .map_err(|_| CalendarWorkflowError::StateUnavailable)?;
        recovery.synchronized_views.insert(view.clone());
        recovery.sync_required_views.remove(view);
        Ok(())
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
                    page_count: round.page_count,
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
        let response = self.provider.calendar_delta_page(view, request).await?;
        let (page, page_bytes) = response.into_parts();
        round.validate_response(&page, page_bytes)?;
        let state = self
            .cache
            .commit_delta_page(view, page, SystemTime::now())
            .await?;
        round.commit(state);
        Ok(round.is_complete())
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
