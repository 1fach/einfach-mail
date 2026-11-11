use super::time::{to_calendar_date, to_offset_date_time};
use std::time::SystemTime;

use einfach_mail_domain::{CalendarViewRange, CalendarViewReference};
use jiff::{Span, Timestamp, civil, tz::TimeZone};

#[derive(Clone)]
enum CalendarTimeZoneChoice {
    System(TimeZone),
    UtcFallback,
}

impl CalendarTimeZoneChoice {
    fn time_zone(&self) -> TimeZone {
        match self {
            Self::System(time_zone) => time_zone.clone(),
            Self::UtcFallback => TimeZone::UTC,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::System(time_zone) => time_zone.iana_name().unwrap_or("UTC"),
            Self::UtcFallback => "UTC",
        }
    }

    fn is_utc_fallback(&self) -> bool {
        matches!(self, Self::UtcFallback)
    }
}

#[derive(Clone)]
pub(crate) struct CalendarWeek {
    week_start: civil::Date,
    time_zone: CalendarTimeZoneChoice,
    view: CalendarViewReference,
}

impl CalendarWeek {
    pub(crate) fn for_system(account_id: &einfach_mail_domain::AccountId) -> Self {
        let system_time_zone = TimeZone::try_system()
            .ok()
            .filter(|time_zone| time_zone.iana_name().is_some());

        if let Some(time_zone) = system_time_zone {
            if let Some(week_start) = current_week_start(&time_zone)
                && let Some(week) = Self::in_system_zone(account_id, week_start, time_zone)
            {
                return week;
            }
            return Self::fixed_utc_fallback(account_id);
        }

        let week_start = current_week_start(&TimeZone::UTC).unwrap_or_else(fallback_week_start);
        Self::in_utc_fallback(account_id, week_start)
            .unwrap_or_else(|| Self::fixed_utc_fallback(account_id))
    }

    pub(crate) fn in_system_zone(
        account_id: &einfach_mail_domain::AccountId,
        week_start: civil::Date,
        time_zone: TimeZone,
    ) -> Option<Self> {
        time_zone.iana_name()?;
        Self::new(
            account_id,
            week_start,
            CalendarTimeZoneChoice::System(time_zone),
        )
    }

    pub(crate) fn in_utc_fallback(
        account_id: &einfach_mail_domain::AccountId,
        week_start: civil::Date,
    ) -> Option<Self> {
        Self::new(account_id, week_start, CalendarTimeZoneChoice::UtcFallback)
    }

    fn new(
        account_id: &einfach_mail_domain::AccountId,
        week_start: civil::Date,
        time_zone: CalendarTimeZoneChoice,
    ) -> Option<Self> {
        if week_start.weekday() != civil::Weekday::Monday {
            return None;
        }
        let view = view_for_week(account_id, week_start, &time_zone.time_zone())?;
        Some(Self {
            week_start,
            time_zone,
            view,
        })
    }

    fn fixed_utc_fallback(account_id: &einfach_mail_domain::AccountId) -> Self {
        Self::in_utc_fallback(account_id, fallback_week_start())
            .expect("a fixed UTC week can be represented by the calendar domain")
    }

    pub(crate) fn with_start(&self, week_start: civil::Date) -> Option<Self> {
        Self::new(self.view.account_id(), week_start, self.time_zone.clone())
    }

    pub(crate) fn start(&self) -> civil::Date {
        self.week_start
    }

    pub(crate) fn view(&self) -> &CalendarViewReference {
        &self.view
    }

    pub(crate) fn time_zone(&self) -> TimeZone {
        self.time_zone.time_zone()
    }

    pub(crate) fn time_zone_label(&self) -> &str {
        self.time_zone.label()
    }

    pub(crate) fn is_utc_fallback(&self) -> bool {
        self.time_zone.is_utc_fallback()
    }
}

fn current_week_start(time_zone: &TimeZone) -> Option<civil::Date> {
    let now = Timestamp::try_from(SystemTime::now()).ok()?;
    monday_start(now.to_zoned(time_zone.clone()).date())
}

fn monday_start(date: civil::Date) -> Option<civil::Date> {
    date.checked_sub(Span::new().days(i64::from(date.weekday().to_monday_zero_offset())))
        .ok()
}

fn fallback_week_start() -> civil::Date {
    civil::Date::new(1970, 1, 5).expect("the Unix week baseline is a valid date")
}

fn view_for_week(
    account_id: &einfach_mail_domain::AccountId,
    week_start: civil::Date,
    time_zone: &TimeZone,
) -> Option<CalendarViewReference> {
    let week_end = week_start.checked_add(Span::new().weeks(1)).ok()?;
    let start_timestamp = week_start.to_zoned(time_zone.clone()).ok()?.timestamp();
    let end_timestamp = week_end.to_zoned(time_zone.clone()).ok()?.timestamp();
    let start = to_offset_date_time(start_timestamp)?;
    let end = to_offset_date_time(end_timestamp)?;
    let start_date = to_calendar_date(week_start)?;
    let end_date = to_calendar_date(week_end)?;
    let range = CalendarViewRange::new(start, end, start_date, end_date).ok()?;
    Some(CalendarViewReference::new(account_id.clone(), range))
}

#[cfg(test)]
mod tests {
    use einfach_mail_domain::AccountId;
    use jiff::{Span, civil, tz::TimeZone};
    use time::Duration;

    use super::{CalendarTimeZoneChoice, CalendarWeek, monday_start, view_for_week};

    #[test]
    fn local_week_bounds_follow_monday_across_a_dst_transition() {
        let time_zone = TimeZone::get("America/New_York")
            .expect("the synthetic IANA zone exists in Linux zoneinfo");
        let sunday = civil::Date::new(2030, 3, 10).expect("synthetic Sunday is valid");
        let start = monday_start(sunday).expect("Monday is in range");
        assert_eq!(start, civil::Date::new(2030, 3, 4).expect("valid date"));
        let account_id =
            AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID");
        let view = view_for_week(&account_id, start, &time_zone)
            .expect("synthetic local week has UTC bounds");
        let range = view.range();

        assert_eq!(range.start_date().day(), 4);
        assert_eq!(range.end_date_exclusive().day(), 11);
        assert_eq!(
            range.end() - range.start(),
            Duration::days(7) - Duration::hours(1)
        );
    }

    #[test]
    fn utc_calendar_week_uses_monday_start_and_utc_offset() {
        let week_start = civil::Date::new(2030, 1, 7).expect("synthetic Monday is valid");
        let account_id =
            AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID");
        assert_eq!(
            monday_start(week_start),
            Some(week_start),
            "Monday dates remain the start of their selected week"
        );
        let view = CalendarWeek::new(
            &account_id,
            week_start,
            CalendarTimeZoneChoice::System(TimeZone::UTC),
        )
        .expect("synthetic UTC week is valid");

        assert_eq!(view.start(), week_start);
        assert_eq!(view.view().range().start().offset(), time::UtcOffset::UTC);
    }

    #[test]
    fn calendar_week_rejects_a_non_monday_start() {
        let week_start = civil::Date::new(2030, 1, 8).expect("synthetic Tuesday is valid");
        let account_id =
            AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID");

        assert!(CalendarWeek::in_utc_fallback(&account_id, week_start).is_none());
    }

    #[test]
    fn navigating_calendar_week_preserves_account_identity() {
        let week_start = civil::Date::new(2030, 1, 7).expect("synthetic Monday is valid");
        let account_id =
            AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID");
        let week = CalendarWeek::in_utc_fallback(&account_id, week_start)
            .expect("synthetic UTC week is valid");
        let next_week_start = week_start
            .checked_add(Span::new().weeks(1))
            .expect("the next synthetic week is in range");

        let next_week = week
            .with_start(next_week_start)
            .expect("the next synthetic UTC week is valid");

        assert_eq!(next_week.view().account_id(), &account_id);
    }
}
