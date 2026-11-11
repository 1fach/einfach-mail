use super::super::time::{to_offset_date_time, to_timestamp};
use super::state::{DraftError, EditableEventFormValues};
use super::*;
use ::time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

pub(super) fn picker_value(date: civil::Date, hour: u32, minute: u32) -> Option<NaiveDateTime> {
    let year = i32::from(date.year());
    NaiveDate::from_ymd_opt(
        year,
        u32::try_from(date.month()).ok()?,
        u32::try_from(date.day()).ok()?,
    )?
    .and_hms_opt(hour, minute, 0)
}

pub(super) fn create_event_draft(
    subject: String,
    local_start: NaiveDateTime,
    local_end: NaiveDateTime,
    time_zone: &TimeZone,
    time_zone_id: CalendarTimeZoneId,
) -> Result<CalendarEventDraft, DraftError> {
    let domain_start = domain_datetime(local_start).ok_or(DraftError::InvalidTime)?;
    let domain_end = domain_datetime(local_end).ok_or(DraftError::InvalidTime)?;
    let start =
        resolve_local_datetime(local_start, time_zone).ok_or(DraftError::AmbiguousLocalTime)?;
    let end = resolve_local_datetime(local_end, time_zone).ok_or(DraftError::AmbiguousLocalTime)?;
    CalendarEventDraft::timed(subject, domain_start, domain_end, time_zone_id, start, end)
        .map_err(|_| DraftError::InvalidTime)
}

pub(super) fn event_endpoints_within_view_week(
    local_start: PrimitiveDateTime,
    local_end: PrimitiveDateTime,
    range: &CalendarViewRange,
) -> bool {
    [local_start.date(), local_end.date()]
        .into_iter()
        .all(|event_date| {
            range.start_date() <= event_date && event_date < range.end_date_exclusive()
        })
}

pub(super) fn selected_picker_time(
    picker: &Entity<DatePickerState>,
    cx: &App,
) -> Result<NaiveDateTime, DraftError> {
    picker
        .read(cx)
        .date_time()
        .start()
        .ok_or(DraftError::MissingDate)
}

pub(super) fn editable_event_picker_values(
    event: &EditableCalendarEvent,
    time_zone: &TimeZone,
) -> Option<EditableEventFormValues> {
    let CalendarOccurrenceTime::Timed { start, end } = event.occurrence().time() else {
        return None;
    };
    Some(EditableEventFormValues {
        subject: event.occurrence().subject().unwrap_or_default().to_owned(),
        start: picker_value_for_offset(*start, time_zone)?,
        end: picker_value_for_offset(*end, time_zone)?,
    })
}

pub(super) fn picker_value_for_offset(
    value: OffsetDateTime,
    time_zone: &TimeZone,
) -> Option<NaiveDateTime> {
    let timestamp = to_timestamp(value)?;
    let local_datetime = timestamp.to_zoned(time_zone.clone()).datetime();
    picker_value(
        local_datetime.date(),
        u32::try_from(local_datetime.hour()).ok()?,
        u32::try_from(local_datetime.minute()).ok()?,
    )
}

pub(super) fn selected_event_time(
    selected: NaiveDateTime,
    original: OffsetDateTime,
    time_zone: &TimeZone,
) -> Result<OffsetDateTime, DraftError> {
    if picker_value_for_offset(original, time_zone) == Some(selected) {
        return Ok(original);
    }
    resolve_local_datetime(selected, time_zone).ok_or(DraftError::AmbiguousLocalTime)
}

pub(super) fn fallback_picker_value(hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1970, 1, 5)
        .expect("the fallback calendar date is valid")
        .and_hms_opt(hour, minute, 0)
        .expect("the fallback calendar time is valid")
}

pub(super) fn domain_datetime(local_date_time: NaiveDateTime) -> Option<PrimitiveDateTime> {
    let month = Month::try_from(u8::try_from(local_date_time.month()).ok()?).ok()?;
    let date = Date::from_calendar_date(
        local_date_time.year(),
        month,
        u8::try_from(local_date_time.day()).ok()?,
    )
    .ok()?;
    let time = Time::from_hms_nano(
        u8::try_from(local_date_time.hour()).ok()?,
        u8::try_from(local_date_time.minute()).ok()?,
        u8::try_from(local_date_time.second()).ok()?,
        local_date_time.nanosecond(),
    )
    .ok()?;
    Some(PrimitiveDateTime::new(date, time))
}

pub(super) fn resolve_local_datetime(
    local_date_time: NaiveDateTime,
    time_zone: &TimeZone,
) -> Option<OffsetDateTime> {
    let date_time = civil::DateTime::new(
        i16::try_from(local_date_time.year()).ok()?,
        i8::try_from(local_date_time.month()).ok()?,
        i8::try_from(local_date_time.day()).ok()?,
        i8::try_from(local_date_time.hour()).ok()?,
        i8::try_from(local_date_time.minute()).ok()?,
        i8::try_from(local_date_time.second()).ok()?,
        i32::try_from(local_date_time.nanosecond()).ok()?,
    )
    .ok()?;
    let timestamp = time_zone
        .to_ambiguous_zoned(date_time)
        .unambiguous()
        .ok()?
        .timestamp();
    to_offset_date_time(timestamp)
}

pub(super) fn format_local_range(draft: &CalendarEventDraft) -> String {
    format!(
        "{} – {} ({})",
        format_local_date_time(draft.local_start()),
        format_local_date_time(draft.local_end()),
        draft.time_zone().as_str()
    )
}

pub(super) fn format_local_date_time(local_date_time: PrimitiveDateTime) -> String {
    let month = u32::from(local_date_time.month() as u8);
    NaiveDate::from_ymd_opt(
        local_date_time.year(),
        month,
        u32::from(local_date_time.day()),
    )
    .and_then(|date| {
        date.and_hms_opt(
            u32::from(local_date_time.hour()),
            u32::from(local_date_time.minute()),
            0,
        )
    })
    .map(|resolved_date_time| resolved_date_time.format("%a, %b %-d · %H:%M").to_string())
    .unwrap_or_else(|| local_date_time.to_string())
}
