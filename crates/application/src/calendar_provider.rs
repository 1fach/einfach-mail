//! Calendar provider requests, responses, and errors used by application workflows.

use std::future::Future;

use einfach_mail_domain::{
    CalendarDeltaLink, CalendarDeltaPage, CalendarEventCreation, CalendarEventUpdate,
    CalendarOccurrence, CalendarViewReference, EditableCalendarEvent,
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
