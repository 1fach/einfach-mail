use time::{Date, Month, PrimitiveDateTime, Time};

use super::{
    CalendarEventCreation, CalendarEventCreationId, CalendarEventDraft, CalendarTimeZoneId,
};
use crate::{CalendarViewRange, CalendarViewReference};

#[test]
fn event_draft_keeps_source_local_values_and_resolved_utc_range_separate() {
    let view = view();
    let local_start = local(2030, Month::January, 7, 9, 30);
    let local_end = local(2030, Month::January, 7, 10, 0);
    let draft = CalendarEventDraft::timed(
        "  Planning  ".to_owned(),
        local_start,
        local_end,
        time_zone("Europe/Berlin"),
        local_start.assume_utc(),
        local_end.assume_utc(),
    )
    .expect("synthetic event draft is valid");
    let creation = CalendarEventCreation::try_new(view.clone(), draft.clone())
        .expect("synthetic creation fits its week");
    let restored = CalendarEventCreation::try_restore(creation.id(), view, draft)
        .expect("synthetic restored event fits its week");

    assert_eq!(restored, creation);
    assert!(!restored.is_legacy_pending());
    assert_eq!(restored.draft().subject(), "Planning");
    assert_eq!(restored.draft().local_start(), local_start);
    assert_eq!(restored.draft().time_zone().as_str(), "Europe/Berlin");
}

#[test]
fn time_zone_identifier_debug_output_is_redacted() {
    let zone = time_zone("Pacific/Chatham");

    let debug = format!("{zone:?}");

    assert_eq!(zone.as_str(), "Pacific/Chatham");
    assert!(
        debug.contains("<redacted>"),
        "missing redaction marker: {debug}"
    );
    assert!(
        !debug.contains("Pacific/Chatham"),
        "Debug output included a time-zone identifier: {debug}"
    );
}

#[test]
fn rejects_creation_endpoints_outside_local_dates_or_utc_view_instants() {
    let time_zone = time_zone("UTC");
    let view = view_for_system_offset(-3);
    let local_start = local(2030, Month::January, 13, 23, 30);
    let local_end = local(2030, Month::January, 14, 0, 30);
    let draft = CalendarEventDraft::timed(
        "Planning".to_owned(),
        local_start,
        local_end,
        time_zone.clone(),
        local_start.assume_utc(),
        local_end.assume_utc(),
    )
    .expect("synthetic event draft is valid");
    let id = CalendarEventCreationId::new();
    assert!(CalendarEventCreation::try_new(view.clone(), draft.clone()).is_err());
    assert!(CalendarEventCreation::try_restore(id, view, draft).is_err());

    let view = view_for_system_offset(3);
    let local_start = local(2030, Month::January, 13, 20, 30);
    let local_end = local(2030, Month::January, 13, 21, 30);
    let draft = CalendarEventDraft::timed(
        "Planning".to_owned(),
        local_start,
        local_end,
        time_zone,
        local_start.assume_utc(),
        local_end.assume_utc(),
    )
    .expect("synthetic event draft is valid");
    let id = CalendarEventCreationId::new();
    assert!(CalendarEventCreation::try_new(view.clone(), draft.clone()).is_err());
    assert!(CalendarEventCreation::try_restore(id, view, draft).is_err());
}

#[test]
fn legacy_pending_creation_can_be_restored_outside_its_view() {
    let view = view();
    let local_start = local(2030, Month::January, 7, 0, 30);
    let local_end = local(2030, Month::January, 7, 1, 0);
    let draft = CalendarEventDraft::timed(
        "Planning".to_owned(),
        local_start,
        local_end,
        time_zone("Pacific/Kiritimati"),
        local(2030, Month::January, 6, 10, 30).assume_utc(),
        local(2030, Month::January, 6, 11, 0).assume_utc(),
    )
    .expect("synthetic legacy event draft is valid");
    let id = CalendarEventCreationId::new();

    assert!(CalendarEventCreation::try_restore(id, view.clone(), draft.clone()).is_err());
    let restored = CalendarEventCreation::restore_legacy_pending(id, view, draft);

    assert_eq!(restored.id(), id);
    assert!(restored.is_legacy_pending());
    assert_eq!(restored.draft().local_start(), local_start);
}

#[test]
fn rejects_blank_subject_and_reversed_local_values() {
    let time_zone = time_zone("UTC");
    let start = local(2030, Month::January, 7, 10, 0);
    let end = local(2030, Month::January, 7, 9, 30);

    assert!(
        CalendarEventDraft::timed(
            "   ".to_owned(),
            start,
            end,
            time_zone.clone(),
            start.assume_utc(),
            end.assume_utc(),
        )
        .is_err()
    );
    assert!(
        CalendarEventDraft::timed(
            "Planning".to_owned(),
            start,
            end,
            time_zone,
            start.assume_utc(),
            end.assume_utc(),
        )
        .is_err()
    );
}

#[test]
fn rejects_empty_time_zone_identifiers() {
    assert!(CalendarTimeZoneId::try_from(String::new()).is_err());
    assert!(CalendarTimeZoneId::try_from("Europe/Bad Zone".to_owned()).is_err());
}

fn local(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> PrimitiveDateTime {
    Date::from_calendar_date(year, month, day)
        .expect("synthetic date is valid")
        .with_time(Time::from_hms(hour, minute, 0).expect("synthetic time is valid"))
}

fn time_zone(value: &str) -> CalendarTimeZoneId {
    CalendarTimeZoneId::try_from(value.to_owned()).expect("synthetic time zone is valid")
}

fn view() -> CalendarViewReference {
    let start = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic date is valid")
        .midnight()
        .assume_utc();
    let end = Date::from_calendar_date(2030, Month::January, 14)
        .expect("synthetic date is valid")
        .midnight()
        .assume_utc();
    CalendarViewReference::new(
        "synthetic-account"
            .to_owned()
            .try_into()
            .expect("account id is valid"),
        CalendarViewRange::new(start, end, start.date(), end.date())
            .expect("synthetic view range is valid"),
    )
}

fn view_for_system_offset(offset_hours: i64) -> CalendarViewReference {
    let start_date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic date is valid");
    let end_date =
        Date::from_calendar_date(2030, Month::January, 14).expect("synthetic date is valid");
    let start = start_date.midnight().assume_utc() - time::Duration::hours(offset_hours);
    let end = end_date.midnight().assume_utc() - time::Duration::hours(offset_hours);
    CalendarViewReference::new(
        "synthetic-account"
            .to_owned()
            .try_into()
            .expect("account id is valid"),
        CalendarViewRange::new(start, end, start_date, end_date)
            .expect("synthetic fixed-offset view range is valid"),
    )
}
