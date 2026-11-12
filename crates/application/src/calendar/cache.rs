//! Calendar cache contracts, cursor state, and cache errors.

use std::{future::Future, time::SystemTime};

use einfach_mail_domain::{
    AccountId, CalendarDeltaLink, CalendarDeltaPage, CalendarEventCreation, CalendarEventUpdate,
    CalendarOccurrence, CalendarViewReference, PendingCalendarEventCreation,
    VersionedCalendarEvent,
};
use thiserror::Error;

use super::provider::CalendarDeltaRequest;

/// A user-safe category for a local calendar cache failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CalendarCacheError {
    /// The encrypted cache is unavailable.
    #[error("the local calendar cache is unavailable")]
    Unavailable,
    /// Another cache handle or lifecycle operation owns an account lock.
    #[error("the local calendar cache is in use")]
    InUse,
    /// Calendar data do not satisfy the cache contract.
    #[error("the local calendar cache contains invalid data")]
    InvalidData,
    /// The requested view belongs to another cache account.
    #[error("the local calendar cache belongs to another account")]
    AccountMismatch,
    /// Another uncertain event creation must be resolved for this account first.
    #[error("another calendar event creation is already pending")]
    CreationAlreadyPending,
}

/// Sync completion and cursor state for one account and fixed calendar view.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarSyncState {
    delta_link: Option<CalendarDeltaLink>,
    next_link: Option<CalendarDeltaLink>,
    last_synchronized_at: Option<SystemTime>,
    mode: CalendarSyncMode,
}

/// Identifies the calendar sync state stored for a fixed view.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarSyncMode {
    /// Continues delta updates against the active snapshot.
    Incremental,
    /// Rebuilds a delta-backed snapshot while leaving the active snapshot visible.
    Replacement,
    /// Marks a complete provider snapshot that has no delta cursor.
    Snapshot,
}

impl CalendarSyncState {
    /// Creates state for a view whose initial delta request has not started.
    #[must_use]
    pub fn initial() -> Self {
        Self {
            delta_link: None,
            next_link: None,
            last_synchronized_at: None,
            mode: CalendarSyncMode::Incremental,
        }
    }

    /// Creates state for a complete snapshot without a provider delta cursor.
    #[must_use]
    pub fn snapshot(completed_at: SystemTime) -> Self {
        Self {
            delta_link: None,
            next_link: None,
            last_synchronized_at: Some(completed_at),
            mode: CalendarSyncMode::Snapshot,
        }
    }

    /// Restores a checkpoint after persistence validates its stored timestamps.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::InvalidData`] when a completed delta state
    /// has no delta link, or a snapshot state has a delta link, next link, or no
    /// completion time.
    pub fn from_checkpoint(
        delta_link: Option<CalendarDeltaLink>,
        next_link: Option<CalendarDeltaLink>,
        last_synchronized_at: Option<SystemTime>,
        mode: CalendarSyncMode,
    ) -> Result<Self, CalendarCacheError> {
        let invalid_checkpoint = match mode {
            CalendarSyncMode::Snapshot => {
                delta_link.is_some() || next_link.is_some() || last_synchronized_at.is_none()
            }
            _ => last_synchronized_at.is_some() && delta_link.is_none(),
        };
        if invalid_checkpoint {
            return Err(CalendarCacheError::InvalidData);
        }
        Ok(Self {
            delta_link,
            next_link,
            last_synchronized_at,
            mode,
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

    /// Returns when the last complete view sync committed.
    #[must_use]
    pub fn last_synchronized_at(&self) -> Option<SystemTime> {
        self.last_synchronized_at
    }

    /// Returns whether the saved cursor belongs to an active replacement scan.
    #[must_use]
    pub fn replacement_active(&self) -> bool {
        self.mode == CalendarSyncMode::Replacement
    }

    /// Returns whether the completed state came from a full snapshot.
    #[must_use]
    pub fn is_snapshot(&self) -> bool {
        self.mode == CalendarSyncMode::Snapshot
    }

    /// Selects the next Graph request, preferring an interrupted page.
    #[must_use]
    pub fn request(&self) -> CalendarDeltaRequest {
        if self.replacement_active() {
            return self
                .next_link
                .as_ref()
                .map_or(CalendarDeltaRequest::Initial, |link| {
                    CalendarDeltaRequest::Continue(link.clone())
                });
        }
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
            .field("mode", &self.mode)
            .finish()
    }
}

/// Encrypted local operations for one account's fixed calendar view.
pub trait CalendarCache: Send + Sync {
    /// Returns the view's saved sync state, creating an empty state when needed.
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

    /// Reports whether any view for the account has an incomplete replacement scan.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account or
    /// [`CalendarCacheError::Unavailable`] when the database cannot be read.
    fn account_has_active_replacement(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<bool, CalendarCacheError>> + Send + '_;

    /// Marks every previously synchronized account view before an event write starts.
    ///
    /// The source view must have a completed sync. The cache keeps each marker
    /// until that view syncs or the account's event write is reconciled.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account,
    /// [`CalendarCacheError::InvalidData`] when the source view has no completed
    /// sync, or [`CalendarCacheError::Unavailable`] when the transaction fails.
    fn mark_calendar_write_sync_required(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;

    /// Reports whether any account view still needs sync after an event write.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account or
    /// [`CalendarCacheError::Unavailable`] when the database cannot be read.
    fn account_has_calendar_write_sync_required(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<bool, CalendarCacheError>> + Send + '_;

    /// Clears all event-write sync markers for an account after a definitive outcome.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account or
    /// [`CalendarCacheError::Unavailable`] when the database cannot be changed.
    fn clear_calendar_write_sync_required(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;

    /// Applies one event page and advances its checkpoint in the same transaction.
    ///
    /// Replacement pages remain staged until their final page commits. Sparse
    /// updates apply only to known occurrences, and out-of-view events stay
    /// staged for later pages but do not appear in the active view. A completed
    /// sync clears only this view's write-recovery marker in that transaction;
    /// partial pages leave the marker unchanged.
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

    /// Replaces the complete cached snapshot for one exact account and view.
    ///
    /// Every occurrence must reference this view, overlap its range, and have a
    /// unique event ID. An empty snapshot removes all events in this view.
    /// Implementations commit the events and completion state atomically.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for a view or occurrence
    /// from another account, [`CalendarCacheError::InvalidData`] for an
    /// occurrence outside this view or a duplicate event ID, or
    /// [`CalendarCacheError::Unavailable`] when the transaction fails.
    fn replace_snapshot(
        &self,
        view: &CalendarViewReference,
        events: Vec<CalendarOccurrence>,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_;

    /// Starts a replacement scan while preserving the active snapshot and cursor.
    ///
    /// `reset_link` must already have passed the provider's authority and range
    /// checks. Starting another reset clears staging data for this exact view.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account,
    /// [`CalendarCacheError::InvalidData`] for malformed stored state or
    /// [`CalendarCacheError::Unavailable`] when the transaction fails.
    fn begin_full_resync(
        &self,
        view: &CalendarViewReference,
        reset_link: Option<CalendarDeltaLink>,
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
    /// The transaction also clears all write-recovery markers for the account.
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
    /// The transaction also clears all write-recovery markers for the account.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarCacheError::AccountMismatch`] for another account or
    /// [`CalendarCacheError::Unavailable`] when the transaction cannot commit.
    fn commit_event_deletion(
        &self,
        event: &VersionedCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_;
}
