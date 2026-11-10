use super::*;

const UPSERT_CALENDAR_EVENT: &str = "INSERT INTO calendar_events (
    account_id,
    range_start_seconds,
    range_start_nanoseconds,
    range_end_seconds,
    range_end_nanoseconds,
    local_start_julian_day,
    local_end_julian_day,
    event_id,
    subject,
    location,
    is_all_day,
    start_seconds,
    start_nanoseconds,
    end_seconds,
    end_nanoseconds,
    start_julian_day,
    end_julian_day,
    original_start_time_zone,
    original_end_time_zone
) VALUES (
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
    ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19
)
ON CONFLICT (
    account_id,
    range_start_seconds,
    range_start_nanoseconds,
    range_end_seconds,
    range_end_nanoseconds,
    local_start_julian_day,
    local_end_julian_day,
    event_id
) DO UPDATE SET
    subject = excluded.subject,
    location = excluded.location,
    is_all_day = excluded.is_all_day,
    start_seconds = excluded.start_seconds,
    start_nanoseconds = excluded.start_nanoseconds,
    end_seconds = excluded.end_seconds,
    end_nanoseconds = excluded.end_nanoseconds,
    start_julian_day = excluded.start_julian_day,
    end_julian_day = excluded.end_julian_day,
    original_start_time_zone = excluded.original_start_time_zone,
    original_end_time_zone = excluded.original_end_time_zone";

const UPSERT_CALENDAR_REBUILD_EVENT: &str = "INSERT INTO calendar_event_rebuild (
    account_id,
    range_start_seconds,
    range_start_nanoseconds,
    range_end_seconds,
    range_end_nanoseconds,
    local_start_julian_day,
    local_end_julian_day,
    event_id,
    is_in_view,
    subject,
    location,
    is_all_day,
    start_seconds,
    start_nanoseconds,
    end_seconds,
    end_nanoseconds,
    start_julian_day,
    end_julian_day,
    original_start_time_zone,
    original_end_time_zone
) VALUES (
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
    ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
)
ON CONFLICT (
    account_id,
    range_start_seconds,
    range_start_nanoseconds,
    range_end_seconds,
    range_end_nanoseconds,
    local_start_julian_day,
    local_end_julian_day,
    event_id
) DO UPDATE SET
    is_in_view = excluded.is_in_view,
    subject = excluded.subject,
    location = excluded.location,
    is_all_day = excluded.is_all_day,
    start_seconds = excluded.start_seconds,
    start_nanoseconds = excluded.start_nanoseconds,
    end_seconds = excluded.end_seconds,
    end_nanoseconds = excluded.end_nanoseconds,
    start_julian_day = excluded.start_julian_day,
    end_julian_day = excluded.end_julian_day,
    original_start_time_zone = excluded.original_start_time_zone,
    original_end_time_zone = excluded.original_end_time_zone";

pub(super) fn write_calendar_event(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event: &CalendarOccurrence,
) -> CacheResult<()> {
    let time_columns = CalendarEventTimeColumns::from_time(event.time());
    connection.execute(
        UPSERT_CALENDAR_EVENT,
        params![
            scope.account_id(),
            scope.range.start_seconds,
            scope.range.start_nanoseconds,
            scope.range.end_seconds,
            scope.range.end_nanoseconds,
            scope.range.local_start_julian_day,
            scope.range.local_end_julian_day,
            event.id().as_str(),
            event.subject(),
            event.location(),
            time_columns.is_all_day,
            time_columns.start_seconds,
            time_columns.start_nanoseconds,
            time_columns.end_seconds,
            time_columns.end_nanoseconds,
            time_columns.start_day,
            time_columns.end_day,
            event.original_start_time_zone(),
            event.original_end_time_zone()
        ],
    )?;
    Ok(())
}

pub(super) fn write_calendar_rebuild_event(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event: &CalendarOccurrence,
) -> CacheResult<()> {
    let time_columns = CalendarEventTimeColumns::from_time(event.time());
    connection.execute(
        UPSERT_CALENDAR_REBUILD_EVENT,
        params![
            scope.account_id(),
            scope.range.start_seconds,
            scope.range.start_nanoseconds,
            scope.range.end_seconds,
            scope.range.end_nanoseconds,
            scope.range.local_start_julian_day,
            scope.range.local_end_julian_day,
            event.id().as_str(),
            i64::from(event.overlaps_view()),
            event.subject(),
            event.location(),
            time_columns.is_all_day,
            time_columns.start_seconds,
            time_columns.start_nanoseconds,
            time_columns.end_seconds,
            time_columns.end_nanoseconds,
            time_columns.start_day,
            time_columns.end_day,
            event.original_start_time_zone(),
            event.original_end_time_zone()
        ],
    )?;
    Ok(())
}

pub(super) fn read_calendar_event(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event_id: &str,
) -> CacheResult<Option<CalendarOccurrence>> {
    read_calendar_event_from(connection, scope, event_id, CalendarEventTable::Active)
}

pub(super) fn read_calendar_event_from(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event_id: &str,
    table: CalendarEventTable,
) -> CacheResult<Option<CalendarOccurrence>> {
    let table_name = match table {
        CalendarEventTable::Active => "calendar_events",
        CalendarEventTable::Rebuild => "calendar_event_rebuild",
    };
    let query = format!(
        "SELECT event_id, subject, location, is_all_day,
                start_seconds, start_nanoseconds, end_seconds, end_nanoseconds,
                start_julian_day, end_julian_day,
                original_start_time_zone, original_end_time_zone
         FROM {table_name}
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7
           AND event_id = ?8"
    );
    let raw = connection
        .query_row(
            &query,
            params![
                scope.account_id(),
                scope.range.start_seconds,
                scope.range.start_nanoseconds,
                scope.range.end_seconds,
                scope.range.end_nanoseconds,
                scope.range.local_start_julian_day,
                scope.range.local_end_julian_day,
                event_id
            ],
            raw_calendar_event_from_row,
        )
        .optional()?;
    raw.map(|raw| decode_calendar_event(raw, scope.view))
        .transpose()
}

pub(super) fn read_calendar_events(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
) -> CacheResult<Vec<CalendarOccurrence>> {
    let mut statement = connection.prepare(
        "SELECT event_id, subject, location, is_all_day,
                start_seconds, start_nanoseconds, end_seconds, end_nanoseconds,
                start_julian_day, end_julian_day,
                original_start_time_zone, original_end_time_zone
         FROM calendar_events
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7
         ORDER BY is_all_day DESC, start_julian_day, start_seconds, event_id",
    )?;
    let raw_events = statement.query_map(
        params![
            scope.account_id(),
            scope.range.start_seconds,
            scope.range.start_nanoseconds,
            scope.range.end_seconds,
            scope.range.end_nanoseconds,
            scope.range.local_start_julian_day,
            scope.range.local_end_julian_day
        ],
        raw_calendar_event_from_row,
    )?;
    raw_events
        .map(|raw| decode_calendar_event(raw?, scope.view))
        .filter_map(|event| match event {
            Ok(event) if event.overlaps_view() => Some(Ok(event)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

struct RawCalendarEvent {
    id: String,
    subject: Option<String>,
    location: Option<String>,
    is_all_day: bool,
    start_seconds: Option<i64>,
    start_nanoseconds: Option<i64>,
    end_seconds: Option<i64>,
    end_nanoseconds: Option<i64>,
    start_julian_day: Option<i32>,
    end_julian_day: Option<i32>,
    original_start_time_zone: Option<String>,
    original_end_time_zone: Option<String>,
}

fn raw_calendar_event_from_row(
    row: &async_sqlite::rusqlite::Row<'_>,
) -> async_sqlite::rusqlite::Result<RawCalendarEvent> {
    Ok(RawCalendarEvent {
        id: row.get(0)?,
        subject: row.get(1)?,
        location: row.get(2)?,
        is_all_day: row.get::<_, i64>(3)? != 0,
        start_seconds: row.get(4)?,
        start_nanoseconds: row.get(5)?,
        end_seconds: row.get(6)?,
        end_nanoseconds: row.get(7)?,
        start_julian_day: row.get(8)?,
        end_julian_day: row.get(9)?,
        original_start_time_zone: row.get(10)?,
        original_end_time_zone: row.get(11)?,
    })
}

fn decode_calendar_event(
    raw: RawCalendarEvent,
    view: &CalendarViewReference,
) -> CacheResult<CalendarOccurrence> {
    let id =
        einfach_mail_domain::EventId::try_from(raw.id).map_err(|_| CacheFailure::InvalidData)?;
    let (start, end) = if raw.is_all_day {
        if raw.start_seconds.is_some()
            || raw.start_nanoseconds.is_some()
            || raw.end_seconds.is_some()
            || raw.end_nanoseconds.is_some()
        {
            return Err(CacheFailure::InvalidData);
        }
        let start = Date::from_julian_day(raw.start_julian_day.ok_or(CacheFailure::InvalidData)?)
            .map_err(|_| CacheFailure::InvalidData)?;
        let end = Date::from_julian_day(raw.end_julian_day.ok_or(CacheFailure::InvalidData)?)
            .map_err(|_| CacheFailure::InvalidData)?;
        (start.midnight().assume_utc(), end.midnight().assume_utc())
    } else {
        if raw.start_julian_day.is_some() || raw.end_julian_day.is_some() {
            return Err(CacheFailure::InvalidData);
        }
        (
            offset_datetime_from_parts(
                raw.start_seconds.ok_or(CacheFailure::InvalidData)?,
                raw.start_nanoseconds.ok_or(CacheFailure::InvalidData)?,
            )?,
            offset_datetime_from_parts(
                raw.end_seconds.ok_or(CacheFailure::InvalidData)?,
                raw.end_nanoseconds.ok_or(CacheFailure::InvalidData)?,
            )?,
        )
    };

    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(raw.subject);
    patch.set_location(raw.location);
    patch.set_original_start_time_zone(raw.original_start_time_zone);
    patch.set_original_end_time_zone(raw.original_end_time_zone);
    let mut time_patch = CalendarOccurrenceTimePatch::new();
    time_patch.set_start(start);
    time_patch.set_end(end);
    time_patch.set_is_all_day(raw.is_all_day);
    patch.set_time(time_patch);
    patch
        .into_new(view.clone(), id)
        .map_err(|_| CacheFailure::InvalidData)
}

pub(super) fn offset_datetime_parts(date_time: OffsetDateTime) -> (i64, i64) {
    (
        date_time.unix_timestamp(),
        i64::from(date_time.nanosecond()),
    )
}

pub(super) fn offset_datetime_from_parts(
    seconds: i64,
    nanoseconds: i64,
) -> CacheResult<OffsetDateTime> {
    if !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(CacheFailure::InvalidData);
    }
    OffsetDateTime::from_unix_timestamp(seconds)
        .and_then(|date_time| date_time.replace_nanosecond(nanoseconds as u32))
        .map_err(|_| CacheFailure::InvalidData)
}
