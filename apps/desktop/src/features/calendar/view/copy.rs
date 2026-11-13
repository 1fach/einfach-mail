use std::time::SystemTime;

use crate::components::relative_age_label;
use einfach_mail_application::{
    CalendarOperationError, CalendarWorkflowError, CalendarWriteReadiness,
};
use einfach_mail_domain::PendingCalendarEventCreation;
use jiff::{Span, civil};

use super::state::DisplayEvent;

pub(super) const UTC_FALLBACK_STATUS_LABEL: &str =
    "The system time zone is unavailable. Timed events are shown in UTC.";

pub(super) struct EventCreationActionCopy {
    pub(super) button_label: &'static str,
    pub(super) sheet_title: &'static str,
}

pub(super) fn event_creation_action_copy(
    pending_creation: Option<&PendingCalendarEventCreation>,
) -> EventCreationActionCopy {
    match pending_creation {
        Some(PendingCalendarEventCreation::Unresolved(_)) => EventCreationActionCopy {
            button_label: "Check event status…",
            sheet_title: "Check event status",
        },
        Some(PendingCalendarEventCreation::Rejected(_)) => EventCreationActionCopy {
            button_label: "Clear rejected draft…",
            sheet_title: "Clear rejected event draft",
        },
        None => EventCreationActionCopy {
            button_label: "New event…",
            sheet_title: "New calendar event",
        },
    }
}

pub(super) fn write_readiness_message(readiness: CalendarWriteReadiness) -> Option<&'static str> {
    match readiness {
        CalendarWriteReadiness::Ready => None,
        CalendarWriteReadiness::ViewSyncRequired => {
            Some("Sync this calendar week before creating, editing, or deleting appointments.")
        }
        CalendarWriteReadiness::AccountSyncRequired => Some(
            "Sync every calendar week already synced for this account before writing calendar changes.",
        ),
    }
}

pub(super) fn calendar_event_accessible_label(
    event: &DisplayEvent<'_>,
    date: civil::Date,
) -> String {
    let subject = event.event.subject().unwrap_or("(No title)");
    match event.event.location() {
        Some(location) => format!(
            "{}, {}, {}, {}",
            date_label(date),
            event.spoken_time_label,
            subject,
            location
        ),
        None => format!(
            "{}, {}, {}",
            date_label(date),
            event.spoken_time_label,
            subject
        ),
    }
}

/// The header title of the week that starts on `start`: its month, or the two months it spans.
pub(super) fn week_title(start: civil::Date) -> String {
    let end = start.saturating_add(Span::new().days(6));
    if start.month() == end.month() && start.year() == end.year() {
        start.strftime("%B %Y").to_string()
    } else if start.year() == end.year() {
        format!("{} – {}", start.strftime("%b"), end.strftime("%b %Y"))
    } else {
        format!("{} – {}", start.strftime("%b %Y"), end.strftime("%b %Y"))
    }
}

/// The day name on a column header, for example "Mon".
pub(super) fn weekday_abbreviation(date: civil::Date) -> String {
    date.strftime("%a").to_string()
}

/// The name of a day column header. Today is named, so it does not rely on its highlight.
pub(super) fn day_header_label(date: civil::Date, is_today: bool) -> String {
    if is_today {
        format!("{}, today", date_label(date))
    } else {
        date_label(date)
    }
}

/// The name of the displayed month in the mini month.
pub(super) fn month_title(month_start: civil::Date) -> String {
    month_start.strftime("%B %Y").to_string()
}

/// The name of one day cell in the mini month.
pub(super) fn full_date_label(date: civil::Date) -> String {
    date.strftime("%A, %B %-d, %Y").to_string()
}

pub(super) fn date_label(date: civil::Date) -> String {
    date.strftime("%A, %B %-d").to_string()
}

pub(super) fn elapsed_label(last_sync: SystemTime) -> String {
    format!("Calendar updated {}.", relative_age_label(last_sync))
}

pub(super) fn calendar_error_message(
    error: CalendarWorkflowError,
    has_cached_events: bool,
) -> String {
    let message = calendar_error_text(error);
    if has_cached_events {
        format!("Showing saved events. {message}")
    } else {
        message.to_owned()
    }
}

pub(super) fn calendar_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::Provider(CalendarOperationError::AccessDenied) => {
            "Microsoft denied calendar access. Review the requested calendar permission or your organization's consent policy."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and connect again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::RateLimited { .. }) => {
            "Microsoft is limiting calendar requests. Wait a moment and sync again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::ResourceLimitExceeded)
        | CalendarWorkflowError::PageLimitExceeded
        | CalendarWorkflowError::ResponseLimitExceeded => {
            "Calendar sync paused after reaching its request limit. Sync again to continue."
        }
        CalendarWorkflowError::RepeatedCursor => {
            "Calendar sync stopped because Microsoft repeated a page. Sync again to retry."
        }
        CalendarWorkflowError::OperationInProgress => {
            "Another mail or calendar operation is active. Wait for it to finish, then sync again."
        }
        CalendarWorkflowError::AccountMismatch => {
            "This calendar view belongs to another account. Open the matching account and try again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::ServiceUnavailable) => {
            "The calendar service is unavailable. Check your connection and sync again."
        }
        _ => "The calendar could not be updated. Try syncing again.",
    }
}
