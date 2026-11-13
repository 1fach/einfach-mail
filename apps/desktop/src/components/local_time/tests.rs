use std::time::SystemTime;

use jiff::{civil, tz::TimeZone};

use super::LocalClock;

fn new_york() -> TimeZone {
    TimeZone::get("America/New_York").expect("the synthetic IANA zone exists in Linux zoneinfo")
}

fn utc_instant(datetime: civil::DateTime) -> SystemTime {
    datetime
        .to_zoned(TimeZone::UTC)
        .expect("the synthetic instant is in range")
        .timestamp()
        .into()
}

#[test]
fn a_fixed_clock_reports_its_pinned_date() {
    let today = civil::date(2026, 11, 13);
    let clock = LocalClock::fixed(TimeZone::UTC, today);

    assert_eq!(clock.today(), today);
}

#[test]
fn local_datetime_uses_the_clock_zone_across_a_utc_day_boundary() {
    let clock = LocalClock::fixed(new_york(), civil::date(2026, 11, 13));
    let instant = utc_instant(civil::date(2026, 11, 13).at(3, 30, 0, 0));

    assert_eq!(
        clock.local_datetime(instant),
        Some(civil::date(2026, 11, 12).at(22, 30, 0, 0)),
        "03:30 UTC is still the evening of the 12th in New York"
    );
}

#[test]
fn the_system_clock_reads_a_date_near_the_current_time() {
    let clock = LocalClock::system();
    let now = clock
        .local_datetime(SystemTime::now())
        .expect("the current time is in range");

    assert!(
        (clock.today() - now.date()).get_days().abs() <= 1,
        "today {:?} and now {:?} agree to within a day",
        clock.today(),
        now.date()
    );
}
