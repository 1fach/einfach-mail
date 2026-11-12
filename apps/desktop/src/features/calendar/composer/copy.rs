use super::state::*;
use super::*;

pub(super) fn rejected_cleanup_error_text(error: CalendarCacheError) -> &'static str {
    match error {
        CalendarCacheError::InUse => {
            "The calendar cache is in use by another operation. Wait for it to finish, then clear the rejected draft."
        }
        CalendarCacheError::Unavailable => {
            "The encrypted calendar cache is unavailable. Restore cache access, then clear the rejected draft."
        }
        CalendarCacheError::AccountMismatch => {
            "The rejected draft belongs to another account. Return to that account before clearing it."
        }
        CalendarCacheError::InvalidData | CalendarCacheError::CreationAlreadyPending => {
            "The saved event draft could not be verified. Refresh the calendar before clearing it."
        }
    }
}

pub(super) fn pending_resolution_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::Cache(CalendarCacheError::Unavailable) => {
            "The encrypted calendar cache is unavailable."
        }
        CalendarWorkflowError::Cache(CalendarCacheError::AccountMismatch) => {
            "The saved event operation belongs to another account."
        }
        _ => "The saved event operation could not be loaded.",
    }
}

pub(super) fn creation_status_text(state: &CreateState) -> Option<&'static str> {
    match state {
        CreateState::Pending {
            status: PendingCreationStatus::Failed(error),
            ..
        } => Some(creation_error_text(*error)),
        CreateState::Completed(CreationOutcome::Confirmed) => {
            Some("Event created and saved to the calendar.")
        }
        CreateState::Pending {
            status: PendingCreationStatus::ConfirmedButNotCached,
            ..
        } => Some(
            "Microsoft confirmed the event, but it could not be saved locally. Retry to recover it safely.",
        ),
        CreateState::Completed(CreationOutcome::Rejected) => {
            Some("Microsoft rejected this event. Correct the details and submit a new event.")
        }
        CreateState::Pending {
            status:
                PendingCreationStatus::RejectedPendingCleanup
                | PendingCreationStatus::RejectedCleanupFailed(_),
            ..
        } => None,
        CreateState::Pending {
            status: PendingCreationStatus::DiscardingRejected,
            ..
        } => Some("Clearing the rejected event draft…"),
        CreateState::Draft
        | CreateState::InvalidDraft(_)
        | CreateState::Pending {
            status: PendingCreationStatus::Ready | PendingCreationStatus::Submitting,
            ..
        }
        | CreateState::PendingResolutionRequired
        | CreateState::PendingResolutionFailed(_) => None,
    }
}

pub(super) fn update_status_text(status: EditStatus) -> Option<&'static str> {
    match status {
        EditStatus::Failed(error) => Some(update_error_text(error)),
        EditStatus::Updated => Some("Event changes were saved to the calendar."),
        EditStatus::UpdatedButNotCached => Some(
            "Microsoft confirmed the changes, but they could not be saved locally. Sync every calendar week already synced for this account before editing another event.",
        ),
        EditStatus::UpdateOutcomeUnknown => Some(
            "Microsoft may have applied these changes. Sync every calendar week already synced for this account before editing another event.",
        ),
        _ => None,
    }
}

pub(super) fn deletion_status_text(status: EditStatus) -> Option<&'static str> {
    match status {
        EditStatus::Deleting => Some("Deleting the appointment…"),
        EditStatus::Deleted => Some("Appointment deleted from the calendar."),
        EditStatus::DeletedButNotCached => Some(
            "Microsoft confirmed the deletion, but the local calendar could not be updated. Sync every calendar week already synced for this account before making another calendar change.",
        ),
        EditStatus::DeleteOutcomeUnknown => Some(
            "Microsoft may have deleted this appointment. Sync every calendar week already synced for this account before making another calendar change.",
        ),
        EditStatus::DeletionFailed(error) => Some(deletion_error_text(error)),
        _ => None,
    }
}

pub(super) fn deletion_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::Provider(CalendarOperationError::EventDeletionRejected) => {
            "Microsoft rejected the deletion. The appointment was not removed."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::EventChanged) => {
            "This appointment changed while the editor was open. Sync every calendar week already synced for this account, then review it again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::EventNotEditable)
        | CalendarWorkflowError::InvalidVersionedEvent => {
            "Microsoft did not confirm that this appointment can be deleted here."
        }
        CalendarWorkflowError::CalendarWriteSyncRequired => {
            "Return to the calendar and sync every calendar week already synced for this account before deleting this appointment."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::AccessDenied) => {
            "Microsoft denied access to delete this appointment. Review the requested permission or your organization's consent policy."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and connect again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::RateLimited { .. }) => {
            "Microsoft is limiting calendar requests. Wait a moment, then try again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::ServiceUnavailable) => {
            "The calendar service is unavailable. Check your connection, then try again."
        }
        CalendarWorkflowError::OperationInProgress => {
            "The calendar is syncing. Try deleting this appointment when sync finishes."
        }
        _ => "The appointment could not be deleted. Check the calendar and try again.",
    }
}

pub(super) fn creation_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::Provider(
            einfach_mail_application::CalendarOperationError::AccessDenied,
        ) => {
            "Microsoft denied access to create this calendar event. Review the requested permission or your organization's consent policy."
        }
        CalendarWorkflowError::Provider(
            einfach_mail_application::CalendarOperationError::SignInRequired,
        ) => "Your Microsoft sign-in needs attention. Return to Accounts and connect again.",
        CalendarWorkflowError::Provider(
            einfach_mail_application::CalendarOperationError::RateLimited { .. },
        ) => "Microsoft is limiting calendar requests. Wait a moment, then retry this event.",
        CalendarWorkflowError::Provider(
            einfach_mail_application::CalendarOperationError::ServiceUnavailable,
        ) => "The calendar service is unavailable. Check your connection, then retry this event.",
        CalendarWorkflowError::OperationInProgress => {
            "The calendar is syncing. Retry this event when sync finishes."
        }
        CalendarWorkflowError::InvalidConfirmation => {
            "Microsoft returned an event that did not match this draft. Retry the saved operation to check the same event."
        }
        CalendarWorkflowError::Cache(error) => cache_creation_error_text(error),
        _ => "The event could not be confirmed. Retry the saved operation to check the same event.",
    }
}

pub(super) fn update_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::CalendarWriteSyncRequired => {
            "Return to the calendar and sync every calendar week already synced for this account before editing this event."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::EventNotEditable) => {
            "Microsoft did not confirm that this event is a standalone appointment that can be edited here."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::EventChanged) => {
            "This event changed while the editor was open. Close this sheet, sync every calendar week already synced for this account, and open Edit again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::UpdateOutcomeUnknown) => {
            "Microsoft may have applied these changes. Sync every calendar week already synced for this account before editing another event."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::AccessDenied) => {
            "Microsoft denied access to update this event. Review the requested permission or your organization's consent policy."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and connect again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::RateLimited { .. }) => {
            "Microsoft is limiting calendar requests. Wait a moment, then save again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::ServiceUnavailable) => {
            "The calendar service is unavailable. Check your connection, then try again."
        }
        CalendarWorkflowError::OperationInProgress => {
            "The calendar is syncing. Save these changes after sync finishes."
        }
        CalendarWorkflowError::InvalidVersionedEvent => {
            "The calendar service returned event details that cannot be edited safely."
        }
        CalendarWorkflowError::InvalidUpdateConfirmation => {
            "Microsoft returned an update that did not match the requested changes. Sync every calendar week already synced for this account before editing another event."
        }
        _ => "The event could not be updated. Check the calendar and try again.",
    }
}

pub(super) fn cache_creation_error_text(error: CalendarCacheError) -> &'static str {
    match error {
        CalendarCacheError::InUse => {
            "The calendar cache is in use by another operation. This event was not sent to Microsoft. Wait for it to finish, then retry."
        }
        CalendarCacheError::Unavailable => {
            "The encrypted calendar cache is unavailable. This event was not sent to Microsoft. Restore cache access, then retry."
        }
        CalendarCacheError::AccountMismatch => {
            "This calendar cache belongs to another account. Return to the matching account before retrying."
        }
        CalendarCacheError::InvalidData => {
            "The event draft could not be saved safely. Microsoft has not received it. Check the event details and retry."
        }
        CalendarCacheError::CreationAlreadyPending => {
            "Another event creation needs to be resolved before a new event can be created."
        }
    }
}

pub(super) fn draft_error_text(error: DraftError) -> &'static str {
    match error {
        DraftError::MissingSubject => "Enter an event title.",
        DraftError::MissingDate => "Choose both start and end dates.",
        DraftError::InvalidTime => "The end must be later than the start.",
        DraftError::AmbiguousLocalTime => {
            "This local time does not exist or occurs twice because of a clock change. Choose another time."
        }
        DraftError::OutsideSelectedWeek => {
            "Choose start and end dates inside the week currently shown in the calendar."
        }
        DraftError::NoChanges => "Change the title or event time before saving.",
    }
}
