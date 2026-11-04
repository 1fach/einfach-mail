//! Calendar provider and cache contracts used by read-only workflows.

use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use einfach_mail_domain::{
    AccountId, CalendarDeltaLink, CalendarDeltaPage, CalendarOccurrence, CalendarViewReference,
};
use thiserror::Error;

use crate::{
    AccountOperationError,
    mail_workflow::{AccountReservationError, ActiveOperations, reserve_calendar_view},
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
    /// The provider denied read access to the calendar.
    #[error("calendar access was denied")]
    AccessDenied,
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
}

/// A user-safe category for a local calendar cache failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CalendarCacheError {
    /// The encrypted cache is unavailable.
    #[error("the local calendar cache is unavailable")]
    Unavailable,
    /// Stored values do not satisfy the calendar domain contract.
    #[error("the local calendar cache contains invalid data")]
    InvalidData,
    /// The requested view belongs to another cache account.
    #[error("the local calendar cache belongs to another account")]
    AccountMismatch,
    /// An unresolved send marker prevents removal of the shared account cache.
    #[error("a pending mail send must be resolved before removing the account cache")]
    PendingMailSend,
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

    /// Removes all account data owned by this cache and its local encryption key.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account,
    /// [`CalendarCacheError::PendingMailSend`] while a send outcome is unresolved,
    /// or [`CalendarCacheError::Unavailable`] when cache removal fails.
    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;
}

/// A user-safe category for a read-only calendar workflow failure.
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

/// Cached occurrences and cursor state loaded for a fixed view range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarSnapshot {
    events: Vec<CalendarOccurrence>,
    sync_state: CalendarSyncState,
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

/// Coordinates calendar delta synchronization and encrypted cached browsing.
pub struct CalendarWorkflow<P, C>
where
    P: CalendarProvider,
    C: CalendarCache,
{
    provider: P,
    cache: C,
    reservations: Arc<Mutex<ActiveOperations>>,
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
        Ok(CalendarSnapshot { events, sync_state })
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
        self.sync_round(view, state).await
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

fn request_cursor(request: &CalendarDeltaRequest) -> Option<CalendarDeltaLink> {
    match request {
        CalendarDeltaRequest::Initial => None,
        CalendarDeltaRequest::Continue(link) => Some(link.clone()),
    }
}

#[cfg(test)]
mod tests;
