use einfach_mail_domain::{CalendarOccurrence, CalendarOccurrenceTime};
use jiff::{Span, Timestamp, civil, tz::TimeZone};
use time::{Date, OffsetDateTime};

use super::super::time::{to_calendar_date, to_timestamp};
use super::CalendarView;
use super::state::{
    AgendaEvent, AgendaEventKey, DayColumn, DisplayEvent, EventSortKey, WeekAgenda, WeekGrid,
};

impl<S> CalendarView<S>
where
    S: einfach_mail_application::CalendarSession + ?Sized + 'static,
{
    pub(super) fn week_agenda(&self) -> WeekAgenda<'_> {
        week_agenda(
            &self.events,
            self.week.start(),
            &self.week.time_zone(),
            self.clock.today(),
        )
    }
}

/// Groups `events` into the seven days that start at `week_start`, ordered day by day and then
/// by time.
pub(super) fn week_agenda<'a>(
    events: &'a [CalendarOccurrence],
    week_start: civil::Date,
    time_zone: &TimeZone,
    today: civil::Date,
) -> WeekAgenda<'a> {
    let mut ordered = Vec::new();
    let days = [0_i64, 1, 2, 3, 4, 5, 6].map(|offset| {
        let date = week_start.saturating_add(Span::new().days(offset));
        let first = ordered.len();
        ordered.extend(
            display_events(events, date, time_zone)
                .into_iter()
                .map(|display| AgendaEvent {
                    key: AgendaEventKey {
                        date,
                        event_id: display.event.id().clone(),
                    },
                    display,
                }),
        );
        DayColumn {
            date,
            is_today: date == today,
            events: first..ordered.len(),
        }
    });
    WeekAgenda {
        events: ordered,
        grid: WeekGrid { days },
    }
}

struct CalendarDayRange {
    date: Date,
    start: Timestamp,
    end: Timestamp,
    time_zone: TimeZone,
}

struct TimedEventDisplayRange {
    sort_start: Timestamp,
    local_start: jiff::Zoned,
    local_end: jiff::Zoned,
    starts_before_day: bool,
    ends_after_day: bool,
}

impl CalendarDayRange {
    fn new(date: civil::Date, time_zone: &TimeZone) -> Option<Self> {
        let domain_date = to_calendar_date(date)?;
        let next_date = date.checked_add(Span::new().days(1)).ok()?;
        let start = date.to_zoned(time_zone.clone()).ok()?.timestamp();
        let end = next_date.to_zoned(time_zone.clone()).ok()?.timestamp();
        Some(Self {
            date: domain_date,
            start,
            end,
            time_zone: time_zone.clone(),
        })
    }
}

impl TimedEventDisplayRange {
    fn new(start: OffsetDateTime, end: OffsetDateTime, day: &CalendarDayRange) -> Option<Self> {
        let start = to_timestamp(start)?;
        let end = to_timestamp(end)?;
        if start >= day.end || end <= day.start {
            return None;
        }
        Some(Self {
            sort_start: if start < day.start { day.start } else { start },
            local_start: start.to_zoned(day.time_zone.clone()),
            local_end: end.to_zoned(day.time_zone.clone()),
            starts_before_day: start < day.start,
            ends_after_day: end > day.end,
        })
    }
}

pub(super) fn display_events<'a>(
    events: &'a [CalendarOccurrence],
    date: civil::Date,
    time_zone: &TimeZone,
) -> Vec<DisplayEvent<'a>> {
    let Some(day_range) = CalendarDayRange::new(date, time_zone) else {
        return Vec::new();
    };
    let mut display_events = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| display_event_for_day(event, index, &day_range))
        .collect::<Vec<_>>();
    display_events.sort_by_key(|display_event| display_event.sort_key);
    display_events
}

/// How one event's time reads on one day: shown without a time zone, and spoken with one.
struct EventTimeLabels {
    shown: String,
    spoken: String,
}

fn display_event_for_day<'a>(
    event: &'a CalendarOccurrence,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<DisplayEvent<'a>> {
    let (sort_key, labels) = match event.time() {
        CalendarOccurrenceTime::AllDay {
            start,
            end_exclusive,
        } => all_day_event_display(*start, *end_exclusive, original_index, day)?,
        CalendarOccurrenceTime::Timed { start, end } => {
            timed_event_display(*start, *end, original_index, day)?
        }
    };
    Some(DisplayEvent {
        event,
        sort_key,
        time_label: labels.shown,
        spoken_time_label: labels.spoken,
    })
}

fn all_day_event_display(
    start: Date,
    end_exclusive: Date,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<(EventSortKey, EventTimeLabels)> {
    if start > day.date || day.date >= end_exclusive {
        return None;
    }
    Some((
        EventSortKey {
            kind: 0,
            seconds: 0,
            nanoseconds: 0,
            original_index,
        },
        EventTimeLabels {
            shown: "All day".to_owned(),
            spoken: "All day".to_owned(),
        },
    ))
}

fn timed_event_display(
    start: OffsetDateTime,
    end: OffsetDateTime,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<(EventSortKey, EventTimeLabels)> {
    let display_range = TimedEventDisplayRange::new(start, end, day)?;
    Some((
        EventSortKey {
            kind: 1,
            seconds: display_range.sort_start.as_second(),
            nanoseconds: display_range.sort_start.subsec_nanosecond(),
            original_index,
        },
        EventTimeLabels {
            shown: timed_event_time_label(&display_range, "%H:%M"),
            spoken: timed_event_time_label(&display_range, "%H:%M %Z"),
        },
    ))
}

fn timed_event_time_label(display_range: &TimedEventDisplayRange, time_format: &str) -> String {
    match (
        display_range.starts_before_day,
        display_range.ends_after_day,
    ) {
        (true, true) => "Continues".to_owned(),
        (true, false) => format!("Until {}", display_range.local_end.strftime(time_format)),
        (false, true) => format!(
            "{} · continues",
            display_range.local_start.strftime(time_format)
        ),
        (false, false) => format!(
            "{}–{}",
            display_range.local_start.strftime(time_format),
            display_range.local_end.strftime(time_format)
        ),
    }
}
