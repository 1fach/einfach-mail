use chrono::{NaiveDate, NaiveDateTime};
use einfach_mail_application::{CalendarCacheError, CalendarOperationError, CalendarWorkflowError};
use einfach_mail_domain::CalendarViewRange;
use jiff::tz::TimeZone;
use time::{Date, Month, PrimitiveDateTime, Time};

use super::copy::creation_error_text;
use super::state::{ComposerState, CreateState, DraftError, EditState};
use super::time::{
    event_endpoints_within_view_week, picker_value_for_offset, resolve_local_datetime,
    selected_event_time,
};

fn local_datetime(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day)
        .expect("synthetic date is valid")
        .and_hms_opt(hour, minute, 0)
        .expect("synthetic local time is valid")
}

#[test]
fn composer_state_keeps_draft_and_recovery_actions_distinct() {
    let draft = ComposerState::Create(CreateState::Draft);
    assert!(draft.show_fields());
    assert!(draft.can_submit());
    assert!(!draft.action_is_terminal());
    assert_eq!(draft.submit_label(), Some("Create event"));

    let invalid_draft =
        ComposerState::Create(CreateState::InvalidDraft(DraftError::MissingSubject));
    assert!(invalid_draft.show_fields());
    assert!(invalid_draft.can_submit());
    assert_eq!(
        invalid_draft.validation_error(),
        Some(DraftError::MissingSubject)
    );

    let unresolved = ComposerState::Create(CreateState::PendingResolutionRequired);
    assert!(unresolved.show_fields());
    assert!(!unresolved.can_submit());
    assert!(unresolved.action_is_terminal());
    assert_eq!(unresolved.submit_label(), None);
}

#[test]
fn failed_edit_load_keeps_the_error_and_permission_review_path() {
    let error = CalendarWorkflowError::Provider(CalendarOperationError::AccessDenied);
    let failed = ComposerState::Edit(EditState::LoadFailed(error));

    assert!(!failed.show_fields());
    assert!(!failed.can_submit());
    assert!(failed.action_is_terminal());
    assert!(failed.requires_calendar_write_permission_review());
    assert_eq!(
        failed.status_text(),
        Some(
            "Microsoft denied access to update this event. Review the requested permission or your organization's consent policy."
        )
    );
}

#[test]
fn edit_loading_hides_fields_and_only_allows_closing_the_sheet() {
    let loading = ComposerState::Edit(EditState::Loading);

    assert!(!loading.show_fields());
    assert!(!loading.can_submit());
    assert!(loading.action_is_terminal());
    assert_eq!(loading.submit_label(), None);
}

#[test]
fn local_time_resolution_rejects_daylight_saving_gaps_and_folds() {
    let new_york = TimeZone::get("America/New_York").expect("the synthetic Linux IANA zone exists");

    assert!(resolve_local_datetime(local_datetime(2030, 3, 10, 2, 30), &new_york).is_none());
    assert!(resolve_local_datetime(local_datetime(2030, 11, 3, 1, 30), &new_york).is_none());
    assert!(resolve_local_datetime(local_datetime(2030, 3, 10, 3, 30), &new_york).is_some());
}

#[test]
fn event_endpoints_must_stay_inside_the_half_open_calendar_week() {
    let week_start =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic week start is valid");
    let week_end = Date::from_calendar_date(2030, Month::January, 14)
        .expect("synthetic exclusive week end is valid");
    let range = CalendarViewRange::new(
        week_start.midnight().assume_utc(),
        week_end.midnight().assume_utc(),
        week_start,
        week_end,
    )
    .expect("synthetic calendar week is valid");
    let first_instant = local_time(week_start, 0, 0);
    let last_instant = local_time(
        Date::from_calendar_date(2030, Month::January, 13)
            .expect("synthetic final week date is valid"),
        23,
        59,
    );

    assert!(event_endpoints_within_view_week(
        first_instant,
        last_instant,
        &range
    ));
    assert!(!event_endpoints_within_view_week(
        local_time(
            Date::from_calendar_date(2030, Month::January, 6)
                .expect("synthetic prior date is valid"),
            23,
            59,
        ),
        first_instant,
        &range,
    ));
    assert!(!event_endpoints_within_view_week(
        first_instant,
        local_time(week_end, 0, 0),
        &range,
    ));
}

#[test]
fn cache_staging_errors_explain_that_the_event_was_not_sent() {
    assert_eq!(
        creation_error_text(CalendarWorkflowError::Cache(
            CalendarCacheError::Unavailable
        )),
        "The encrypted calendar cache is unavailable. This event was not sent to Microsoft. Restore cache access, then retry."
    );
    assert_eq!(
        creation_error_text(CalendarWorkflowError::Cache(
            CalendarCacheError::InvalidData
        )),
        "The event draft could not be saved safely. Microsoft has not received it. Check the event details and retry."
    );
}

#[test]
fn an_unchanged_minute_preserves_hidden_event_seconds() {
    let time_zone = TimeZone::UTC;
    let original = Date::from_calendar_date(2030, Month::January, 8)
        .expect("synthetic date is valid")
        .with_time(Time::from_hms_nano(9, 30, 45, 123_000_000).expect("valid time"))
        .assume_utc();
    let selected = picker_value_for_offset(original, &time_zone)
        .expect("synthetic time is representable by the minute picker");

    assert_eq!(
        selected_event_time(selected, original, &time_zone),
        Ok(original)
    );
}

#[test]
fn a_changed_local_time_resolves_against_the_selected_zone() {
    let time_zone = TimeZone::get("America/New_York")
        .expect("the synthetic IANA zone exists in Linux zoneinfo");
    let original = Date::from_calendar_date(2030, Month::January, 8)
        .expect("synthetic date is valid")
        .with_time(Time::from_hms(14, 0, 0).expect("valid time"))
        .assume_utc();
    let selected = local_datetime(2030, 1, 8, 10, 0);

    let updated =
        selected_event_time(selected, original, &time_zone).expect("synthetic local time resolves");

    assert_eq!(updated.unix_timestamp(), original.unix_timestamp() + 3_600);
}

#[test]
fn an_edit_to_a_daylight_saving_gap_is_rejected() {
    let time_zone = TimeZone::get("America/New_York")
        .expect("the synthetic IANA zone exists in Linux zoneinfo");
    let original = Date::from_calendar_date(2030, Month::March, 10)
        .expect("synthetic date is valid")
        .with_time(Time::from_hms(6, 30, 0).expect("valid time"))
        .assume_utc();

    assert_eq!(
        selected_event_time(local_datetime(2030, 3, 10, 2, 30), original, &time_zone),
        Err(DraftError::AmbiguousLocalTime)
    );
}

fn local_time(date: Date, hour: u8, minute: u8) -> PrimitiveDateTime {
    date.with_hms(hour, minute, 0)
        .expect("synthetic local time is valid")
}
