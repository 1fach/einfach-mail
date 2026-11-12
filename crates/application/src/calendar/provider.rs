//! Calendar provider requests, responses, and errors used by application workflows.

use std::future::Future;

use einfach_mail_domain::{
    CalendarDeltaLink, CalendarDeltaPage, CalendarEventCreation, CalendarEventUpdate,
    CalendarOccurrence, CalendarViewReference, VersionedCalendarEvent,
};
use thiserror::Error;

use crate::AccountOperationError;

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
    /// The connected account does not expose calendar operations.
    #[error("this account does not support calendar operations")]
    UnsupportedOperation,
    /// The provider confirmed that the event request was rejected.
    #[error("the calendar event request was rejected")]
    EventCreationRejected,
    /// The provider confirmed that the event update was rejected.
    #[error("the calendar event update was rejected")]
    EventUpdateRejected,
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
    /// The provider may have applied the update, but the client did not receive confirmation.
    #[error("the calendar update outcome is unknown")]
    UpdateOutcomeUnknown,
    /// The provider rejected the confirmed event deletion request.
    #[error("the calendar event deletion was rejected")]
    EventDeletionRejected,
    /// The provider may have deleted the event, but the client did not receive confirmation.
    #[error("the calendar deletion outcome is unknown")]
    DeleteOutcomeUnknown,
}

impl From<AccountOperationError> for CalendarOperationError {
    fn from(error: AccountOperationError) -> Self {
        match error {
            AccountOperationError::AccessDenied => Self::AccessDenied,
            AccountOperationError::UnsupportedOperation => Self::UnsupportedOperation,
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

/// Selects the synchronization protocol used by one calendar provider.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarSyncStrategy {
    /// Synchronizes changes with resumable delta pages.
    Delta,
    /// Replaces the view with one complete provider snapshot.
    Snapshot,
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

    /// Returns the parsed page and response size.
    #[must_use]
    pub fn into_parts(self) -> (CalendarDeltaPage, usize) {
        (self.page, self.response_bytes)
    }
}

/// A complete fixed-range snapshot returned by a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarViewSnapshotResponse {
    occurrences: Vec<CalendarOccurrence>,
    response_bytes: usize,
}

impl CalendarViewSnapshotResponse {
    /// Creates a snapshot with its occurrences and received response size.
    #[must_use]
    pub fn new(occurrences: Vec<CalendarOccurrence>, response_bytes: usize) -> Self {
        Self {
            occurrences,
            response_bytes,
        }
    }

    /// Returns the occurrences and response size.
    #[must_use]
    pub fn into_parts(self) -> (Vec<CalendarOccurrence>, usize) {
        (self.occurrences, self.response_bytes)
    }
}

/// One calendar delta page or a provider-confirmed expired cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalendarDeltaResponse {
    /// Contains event changes and the next-page or completed delta link.
    Page(CalendarDeltaPageResponse),
    /// Reports an expired cursor.
    /// When `reset_link` is `None`, the workflow resumes with the fixed-range
    /// initial query.
    Reset {
        /// The validated replacement URL returned by the provider, if any.
        reset_link: Option<CalendarDeltaLink>,
        /// Bytes read from the reset response body or its `Location` header.
        response_bytes: usize,
    },
}

/// Calendar operations required by the application.
pub trait CalendarProvider: Send + Sync {
    /// Returns the synchronization protocol for the active account session.
    ///
    /// # Errors
    ///
    /// Returns a user-safe provider error when an account router cannot
    /// determine the selected provider.
    fn calendar_sync_strategy(
        &self,
    ) -> impl Future<Output = Result<CalendarSyncStrategy, CalendarOperationError>> + Send + '_;

    /// Loads one delta page or expired-cursor reset for a primary-calendar view.
    ///
    /// The provider rejects a view scoped to another account. Continuation
    /// links are opaque and are validated before the user's token is attached.
    ///
    /// # Errors
    ///
    /// Returns a reset only for an expired continuation cursor. Returns a
    /// user-safe provider error when sign-in, access, transport, response
    /// validation, or resource limits prevent loading the response.
    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_;

    /// Loads a complete snapshot for one fixed primary-calendar view.
    ///
    /// Providers that use delta synchronization can keep the default unsupported
    /// result. Snapshot providers must reject incomplete responses.
    ///
    /// # Errors
    ///
    /// Returns a user-safe provider error when sign-in, access, transport,
    /// response validation, or resource limits prevent a complete snapshot.
    fn calendar_view_snapshot(
        &self,
        _view: &CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarViewSnapshotResponse, CalendarOperationError>> + Send + '_
    {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    /// Sends a newly staged event creation to the provider.
    ///
    /// The workflow calls this method only when the cache reports
    /// [`crate::CalendarEventCreationStage::NewlyStaged`]. If the operation
    /// remains unresolved, the workflow calls
    /// [`Self::recover_event_creation`] with the same operation.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarOperationError::EventCreationRejected`] only when a
    /// provider response proves it did not create the event. Returns
    /// [`CalendarOperationError::CreationOutcomeUnknown`] when the request may
    /// have reached the provider without confirmation. Other errors describe sign-in,
    /// access, transport, or response validation failures.
    fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_;

    /// Resolves the outcome of an event creation already stored by the cache.
    ///
    /// Implementations must use their provider's recovery policy for the exact
    /// operation and must not change its identifier or draft. A provider that
    /// cannot recover an existing creation returns
    /// [`CalendarOperationError::UnsupportedOperation`].
    ///
    /// # Errors
    ///
    /// Returns [`CalendarOperationError::EventCreationRejected`] only when the
    /// provider proves that the original creation was rejected. Returns
    /// [`CalendarOperationError::CreationOutcomeUnknown`] when the event may
    /// exist without confirmation. Other errors describe sign-in, access,
    /// transport, or response-validation failures.
    fn recover_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_;

    /// Loads the provider version for an event used by calendar write operations.
    ///
    /// The provider must reject events outside its supported write shape and
    /// return its version with the complete current occurrence values.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarOperationError::EventNotEditable`] when the event is
    /// outside the provider's supported write shape, or a user-safe provider
    /// error when the event cannot be read or its response cannot be validated.
    fn load_versioned_event(
        &self,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<VersionedCalendarEvent, CalendarOperationError>> + Send + '_;

    /// Rechecks the captured event version and updates its requested fields.
    ///
    /// Implementations must not automatically retry the provider write.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarOperationError::EventChanged`] when the version is
    /// stale, [`CalendarOperationError::UpdateOutcomeUnknown`] when the write
    /// may have reached the provider without a verifiable response, or another
    /// user-safe provider error when the provider rejects the request.
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
    /// when the provider may have deleted it without a verifiable response, or
    /// another user-safe provider error when the provider rejects the request.
    fn delete_event(
        &self,
        event: &VersionedCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_;
}
