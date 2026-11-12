use time::{Date, Month, OffsetDateTime};

use crate::{
    AccountId, CalendarOccurrencePatch, CalendarViewRange, CalendarViewReference, EventId,
};

use super::{
    CalendarEventUpdate, CalendarEventUpdateDraft, CalendarEventVersion,
    InvalidCalendarEventVersion, NoCalendarEventChanges, VersionedCalendarEvent,
};

#[test]
fn validates_event_versions_and_hides_them_in_debug() {
    let version =
        CalendarEventVersion::try_from("version-1".to_owned()).expect("synthetic version is valid");

    assert_eq!(version.as_str(), "version-1");
    assert!(!format!("{version:?}").contains("version-1"));
    assert_eq!(
        CalendarEventVersion::try_from(String::new()),
        Err(InvalidCalendarEventVersion)
    );
    assert_eq!(
        CalendarEventVersion::try_from("v".repeat(4097)),
        Err(InvalidCalendarEventVersion)
    );
    assert_eq!(
        CalendarEventVersion::try_from("version\n1".to_owned()),
        Err(InvalidCalendarEventVersion)
    );
}

#[test]
fn update_draft_rejects_control_characters_in_the_subject() {
    assert!(
        CalendarEventUpdateDraft::timed(
            "Planning\tmeeting".to_owned(),
            timestamp(9),
            timestamp(10),
        )
        .is_err()
    );
}

#[test]
fn update_requires_a_changed_subject_or_timed_range() {
    let event = occurrence("Planning", primary_calendar_view());
    let versioned_event = VersionedCalendarEvent::new(
        event,
        CalendarEventVersion::try_from("version-1".to_owned()).expect("synthetic version is valid"),
    )
    .expect("synthetic timed event is editable");
    let draft = CalendarEventUpdateDraft::timed("Planning".to_owned(), timestamp(9), timestamp(10))
        .expect("synthetic update draft is valid");

    assert_eq!(
        CalendarEventUpdate::new(versioned_event, draft),
        Err(NoCalendarEventChanges)
    );
}

#[test]
fn versioned_event_preserves_identity_and_version_in_an_update() {
    let event = occurrence("Planning", primary_calendar_view());
    let event_id = event.id().clone();
    let versioned_event = VersionedCalendarEvent::new(
        event,
        CalendarEventVersion::try_from("version-1".to_owned()).expect("synthetic version is valid"),
    )
    .expect("synthetic timed event can be versioned");
    let draft = CalendarEventUpdateDraft::timed(
        "Updated planning".to_owned(),
        timestamp(11),
        timestamp(12),
    )
    .expect("synthetic update draft is valid");
    let update = CalendarEventUpdate::new(versioned_event, draft).expect("event values changed");

    assert_eq!(update.source().occurrence().id(), &event_id);
    assert_eq!(update.source().version().as_str(), "version-1");
}

#[test]
fn confirmation_rejects_a_different_account() {
    let update = event_update();
    let event = occurrence_with_confirmation_fields(
        view_for_account("synthetic-other-account"),
        "synthetic-event",
        "Updated planning",
        (timestamp(11), timestamp(12)),
    );

    assert!(!update.confirms(&event));
}

#[test]
fn confirmation_rejects_a_different_view_range() {
    let update = event_update();
    let event = occurrence_with_confirmation_fields(
        view_with_shifted_range(),
        "synthetic-event",
        "Updated planning",
        (timestamp(11), timestamp(12)),
    );

    assert!(!update.confirms(&event));
}

#[test]
fn confirmation_rejects_a_different_event_id() {
    let update = event_update();
    let event = occurrence_with_confirmation_fields(
        primary_calendar_view(),
        "synthetic-other-event",
        "Updated planning",
        (timestamp(11), timestamp(12)),
    );

    assert!(!update.confirms(&event));
}

#[test]
fn confirmation_rejects_a_different_subject() {
    let update = event_update();
    let event = occurrence_with_confirmation_fields(
        primary_calendar_view(),
        "synthetic-event",
        "Different planning",
        (timestamp(11), timestamp(12)),
    );

    assert!(!update.confirms(&event));
}

#[test]
fn confirmation_rejects_a_different_time_range() {
    let update = event_update();
    let event = occurrence_with_confirmation_fields(
        primary_calendar_view(),
        "synthetic-event",
        "Updated planning",
        (timestamp(9), timestamp(10)),
    );

    assert!(!update.confirms(&event));
}

#[test]
fn confirmation_accepts_the_same_event_and_requested_values() {
    let update = event_update();
    let event = occurrence_with_confirmation_fields(
        primary_calendar_view(),
        "synthetic-event",
        "Updated planning",
        (timestamp(11), timestamp(12)),
    );

    assert!(update.confirms(&event));
}

fn event_update() -> CalendarEventUpdate {
    let view = primary_calendar_view();
    let source = VersionedCalendarEvent::new(
        occurrence("Planning", view),
        CalendarEventVersion::try_from("version-1".to_owned()).expect("synthetic version is valid"),
    )
    .expect("synthetic timed event is editable");
    let draft = CalendarEventUpdateDraft::timed(
        "Updated planning".to_owned(),
        timestamp(11),
        timestamp(12),
    )
    .expect("synthetic update draft is valid");
    CalendarEventUpdate::new(source, draft).expect("values changed")
}

fn occurrence(subject: &str, range: CalendarViewReference) -> crate::CalendarOccurrence {
    occurrence_with_confirmation_fields(
        range,
        "synthetic-event",
        subject,
        (timestamp(9), timestamp(10)),
    )
}

fn occurrence_with_confirmation_fields(
    reference: CalendarViewReference,
    id: &str,
    subject: &str,
    time_range: (OffsetDateTime, OffsetDateTime),
) -> crate::CalendarOccurrence {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(subject.to_owned()));
    let (start, end) = time_range;
    let mut time = crate::CalendarOccurrenceTimePatch::new();
    time.set_start(start);
    time.set_end(end);
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
        .into_new(
            reference,
            EventId::try_from(id.to_owned()).expect("synthetic event ID is valid"),
        )
        .expect("synthetic event is complete")
}

fn primary_calendar_view() -> CalendarViewReference {
    view_for_account("synthetic-account")
}

fn view_for_account(account_id: &str) -> CalendarViewReference {
    let start_date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic start date is valid");
    let end_date =
        Date::from_calendar_date(2030, Month::January, 14).expect("synthetic end date is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic week is valid");
    CalendarViewReference::new(
        AccountId::try_from(account_id.to_owned()).expect("synthetic account ID is valid"),
        range,
    )
}

fn view_with_shifted_range() -> CalendarViewReference {
    let view = primary_calendar_view();
    let source_range = view.range();
    let range = CalendarViewRange::new(
        source_range.start() + time::Duration::hours(1),
        source_range.end(),
        source_range.start_date(),
        source_range.end_date_exclusive(),
    )
    .expect("synthetic shifted week is valid");
    CalendarViewReference::new(view.account_id().clone(), range)
}

fn timestamp(hour: u8) -> OffsetDateTime {
    Date::from_calendar_date(2030, Month::January, 8)
        .expect("synthetic date is valid")
        .with_hms(hour, 0, 0)
        .expect("synthetic time is valid")
        .assume_utc()
}
