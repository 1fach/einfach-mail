use einfach_mail_domain::{CalendarOccurrence, CalendarOccurrenceTime};
use jiff::{Span, Timestamp, civil, tz::TimeZone};
use time::{Date, OffsetDateTime};

use super::super::time::{to_calendar_date, to_timestamp};
use super::CalendarView;
use super::state::{AgendaEvent, AgendaEventKey, AgendaRow, DisplayEvent, EventSortKey};

impl<S> CalendarView<S>
where
    S: einfach_mail_application::CalendarSession + ?Sized + 'static,
{
    pub(super) fn agenda_rows(&self) -> Vec<AgendaRow<'_>> {
        let mut rows = Vec::new();
        let time_zone = self.week.time_zone();
        for offset in 0..7_i64 {
            let Some(date) = self.week.start().checked_add(Span::new().days(offset)).ok() else {
                continue;
            };
            rows.push(AgendaRow::DayHeading(date));
            let day_events = display_events(&self.events, date, &time_zone);
            if day_events.is_empty() {
                rows.push(AgendaRow::EmptyDay);
            } else {
                rows.extend(day_events.into_iter().map(|display| {
                    AgendaRow::Event(AgendaEvent {
                        key: AgendaEventKey {
                            date,
                            event_id: display.event.id().clone(),
                        },
                        display,
                    })
                }));
            }
        }
        rows
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

fn display_event_for_day<'a>(
    event: &'a CalendarOccurrence,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<DisplayEvent<'a>> {
    let (sort_key, time_label) = match event.time() {
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
        time_label,
    })
}

fn all_day_event_display(
    start: Date,
    end_exclusive: Date,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<(EventSortKey, String)> {
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
        "All day".to_owned(),
    ))
}

fn timed_event_display(
    start: OffsetDateTime,
    end: OffsetDateTime,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<(EventSortKey, String)> {
    let display_range = TimedEventDisplayRange::new(start, end, day)?;
    let time_label = timed_event_time_label(&display_range);
    Some((
        EventSortKey {
            kind: 1,
            seconds: display_range.sort_start.as_second(),
            nanoseconds: display_range.sort_start.subsec_nanosecond(),
            original_index,
        },
        time_label,
    ))
}

fn timed_event_time_label(display_range: &TimedEventDisplayRange) -> String {
    match (
        display_range.starts_before_day,
        display_range.ends_after_day,
    ) {
        (true, true) => "Continues".to_owned(),
        (true, false) => format!("Until {}", display_range.local_end.strftime("%H:%M %Z")),
        (false, true) => format!(
            "{} · continues",
            display_range.local_start.strftime("%H:%M %Z")
        ),
        (false, false) => format!(
            "{}–{}",
            display_range.local_start.strftime("%H:%M %Z"),
            display_range.local_end.strftime("%H:%M %Z")
        ),
    }
}
