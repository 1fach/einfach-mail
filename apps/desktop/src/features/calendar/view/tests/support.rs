use super::super::super::week::CalendarWeek;
use crate::test_support::calendar::profile;
use einfach_mail_domain::CalendarViewReference;
use jiff::{civil, tz::TimeZone};

pub(super) fn system_calendar_week(week_start: civil::Date, time_zone: TimeZone) -> CalendarWeek {
    CalendarWeek::in_system_zone(profile().id(), week_start, time_zone)
        .expect("synthetic system calendar week is valid")
}

pub(super) fn utc_fallback_calendar_week(week_start: civil::Date) -> CalendarWeek {
    CalendarWeek::in_utc_fallback(profile().id(), week_start)
        .expect("synthetic UTC fallback calendar week is valid")
}

pub(super) fn system_calendar_view(week_start: civil::Date) -> CalendarViewReference {
    system_calendar_week(week_start, TimeZone::UTC)
        .view()
        .clone()
}
