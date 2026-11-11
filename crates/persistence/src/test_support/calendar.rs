use einfach_mail_application::{CalendarDeltaPageResponse, CalendarDeltaResponse};
use einfach_mail_domain::{
    AccountId, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress, CalendarEventVersion,
    CalendarOccurrencePatch, CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
    EditableCalendarEvent, EventId,
};
use time::{Date, Month};

pub(crate) fn calendar_view(account: &AccountId) -> CalendarViewReference {
    let start_date = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic calendar view start is valid");
    let end_date = start_date
        .checked_add(time::Duration::days(7))
        .expect("synthetic calendar view end is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic calendar view range is valid");
    CalendarViewReference::new(account.clone(), range)
}

pub(crate) fn completed_calendar_page(cursor: &str) -> CalendarDeltaResponse {
    CalendarDeltaResponse::Page(CalendarDeltaPageResponse::new(
        CalendarDeltaPage::new(
            Vec::new(),
            CalendarDeltaProgress::Complete(CalendarDeltaLink::from_opaque(format!(
                "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken={cursor}"
            ))),
        ),
        128,
    ))
}

pub(crate) fn editable_calendar_event(view: CalendarViewReference) -> EditableCalendarEvent {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Synthetic event".to_owned()));
    let start = view.range().start() + time::Duration::hours(1);
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(start);
    time.set_end(start + time::Duration::hours(1));
    time.set_is_all_day(false);
    patch.set_time(time);
    let occurrence = patch
        .into_new(
            view,
            EventId::try_from("synthetic-event".to_owned()).expect("synthetic event ID is valid"),
        )
        .expect("synthetic calendar event is complete");
    let version = CalendarEventVersion::try_from("synthetic-version".to_owned())
        .expect("synthetic calendar version is valid");
    EditableCalendarEvent::new(occurrence, version)
        .expect("synthetic timed calendar event is editable")
}
