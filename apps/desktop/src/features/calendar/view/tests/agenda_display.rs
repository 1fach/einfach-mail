use super::*;

#[test]
fn timed_events_that_cross_midnight_appear_on_each_occupied_day() {
    let event = timed_event(
        Date::from_calendar_date(2030, Month::January, 6)
            .expect("synthetic date is valid")
            .with_time(time::Time::from_hms(23, 0, 0).expect("synthetic time is valid"))
            .assume_utc(),
        Date::from_calendar_date(2030, Month::January, 8)
            .expect("synthetic date is valid")
            .with_time(time::Time::from_hms(10, 0, 0).expect("synthetic time is valid"))
            .assume_utc(),
    );
    let time_zone = TimeZone::UTC;
    let first_day = civil::Date::new(2030, 1, 7).expect("synthetic date is valid");
    let last_day = civil::Date::new(2030, 1, 8).expect("synthetic date is valid");

    let first_day_events =
        super::display_events(std::slice::from_ref(&event), first_day, &time_zone);
    let last_day_events = super::display_events(std::slice::from_ref(&event), last_day, &time_zone);

    assert_eq!(first_day_events.len(), 1);
    assert_eq!(first_day_events[0].time_label, "Continues");
    assert_eq!(last_day_events.len(), 1);
    assert_eq!(last_day_events[0].time_label, "Until 10:00 UTC");
    assert!(matches!(event.time(), CalendarOccurrenceTime::Timed { .. }));
}
