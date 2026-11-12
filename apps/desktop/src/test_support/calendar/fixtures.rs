use einfach_mail_domain::{
    AccountCapabilities, AccountId, AccountProfile, CalendarCapabilities, CalendarOccurrence,
    CalendarOccurrencePatch, CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
    EventId, MailCapabilities,
};
use gpui_kit::{Modifiers, VisualTestContext};
use time::{Date, Month, OffsetDateTime};

use super::super::{synthetic_account_id, synthetic_account_profile};

pub(crate) fn profile() -> AccountProfile {
    synthetic_account_profile(
        synthetic_account_id(),
        None,
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid")
}

pub(crate) fn read_only_calendar_profile() -> AccountProfile {
    profile_with_calendar_capabilities(CalendarCapabilities::view_only())
}

pub(crate) fn profile_with_calendar_capabilities(calendar: CalendarCapabilities) -> AccountProfile {
    synthetic_account_profile(
        synthetic_account_id(),
        None,
        AccountCapabilities::new(MailCapabilities::read_only(), calendar),
    )
    .expect("synthetic calendar profile is valid")
}

pub(crate) fn workflow_account_id() -> AccountId {
    synthetic_account_id()
}

pub(crate) fn timed_event(start: OffsetDateTime, end: OffsetDateTime) -> CalendarOccurrence {
    timed_event_with_id("synthetic-timed-event", start, end)
}

pub(crate) fn timed_event_with_id(
    event_id: &str,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> CalendarOccurrence {
    let account = synthetic_account_id();
    let start_date = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic view start date is valid");
    let end_date = Date::from_calendar_date(2030, Month::January, 14)
        .expect("synthetic view end date is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic calendar range is valid");
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Synthetic overnight event".to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(start);
    time.set_end(end);
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
        .into_new(
            CalendarViewReference::new(account, range),
            EventId::try_from(event_id.to_owned()).expect("synthetic event ID is valid"),
        )
        .expect("synthetic timed event is valid")
}

pub(crate) fn agenda_events(count: usize) -> Vec<CalendarOccurrence> {
    let date = Date::from_calendar_date(2030, Month::January, 7).expect("synthetic date is valid");
    (0..count)
        .map(|index| {
            let event_id = format!("synthetic-calendar-event-{}", index + 1);
            let start_minutes = 8 * 60 + i32::try_from(index).expect("event index fits") * 5;
            let start = date.midnight().assume_utc().replace_time(
                time::Time::from_hms(
                    u8::try_from(start_minutes / 60).expect("hour fits"),
                    u8::try_from(start_minutes % 60).expect("minute fits"),
                    0,
                )
                .expect("synthetic event time is valid"),
            );
            timed_event_with_id(&event_id, start, start + time::Duration::minutes(30))
        })
        .collect()
}

pub(crate) fn draw_calendar_view(context: &mut VisualTestContext) {
    context.update(|window, app| {
        let _ = window.draw(app);
    });
}

pub(crate) fn select_default_timed_event(context: &mut VisualTestContext) {
    let event = context
        .debug_bounds("calendar-event-row-2030-01-09-synthetic-timed-event")
        .expect("the default calendar cache contains a timed event");
    context.simulate_click(event.center(), Modifiers::default());
}
