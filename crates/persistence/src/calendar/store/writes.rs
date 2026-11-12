use super::*;

pub(super) fn calendar_account_has_active_replacement(
    connection: &Connection,
    account_id: &str,
) -> CacheResult<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(
                SELECT 1 FROM calendar_view_sync_state
                WHERE account_id = ?1 AND replacement_active = 1
            )",
        [account_id],
        |row| row.get(0),
    )?)
}

pub(super) fn calendar_account_has_write_sync_required(
    connection: &Connection,
    account_id: &str,
) -> CacheResult<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(
                SELECT 1 FROM calendar_view_sync_state
                WHERE account_id = ?1 AND write_sync_required = 1
            )",
        [account_id],
        |row| row.get(0),
    )?)
}

pub(super) fn mark_calendar_write_sync_required(
    transaction: &Transaction<'_>,
    account_id: &str,
    source_scope: CalendarScope,
) -> CacheResult<()> {
    let source_is_synchronized = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM calendar_view_sync_state
            WHERE account_id = ?1
              AND range_start_seconds = ?2
              AND range_start_nanoseconds = ?3
              AND range_end_seconds = ?4
              AND range_end_nanoseconds = ?5
              AND local_start_julian_day = ?6
              AND local_end_julian_day = ?7
              AND last_sync_seconds IS NOT NULL
        )",
        params![
            account_id,
            source_scope.start_seconds,
            source_scope.start_nanoseconds,
            source_scope.end_seconds,
            source_scope.end_nanoseconds,
            source_scope.local_start_julian_day,
            source_scope.local_end_julian_day,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if !source_is_synchronized {
        return Err(CacheFailure::InvalidData);
    }
    transaction.execute(
        "UPDATE calendar_view_sync_state
         SET write_sync_required = 1
         WHERE account_id = ?1 AND last_sync_seconds IS NOT NULL",
        [account_id],
    )?;
    Ok(())
}

pub(super) fn clear_calendar_write_sync_required(
    transaction: &Connection,
    account_id: &str,
) -> CacheResult<()> {
    transaction.execute(
        "UPDATE calendar_view_sync_state
         SET write_sync_required = 0 WHERE account_id = ?1",
        [account_id],
    )?;
    Ok(())
}

struct RawPendingEventCreation {
    range_start_seconds: i64,
    range_start_nanoseconds: i64,
    range_end_seconds: i64,
    range_end_nanoseconds: i64,
    local_start_julian_day: i32,
    local_end_julian_day: i32,
    creation_id: String,
    subject: String,
    local_start_seconds: i64,
    local_start_nanoseconds: i64,
    local_end_seconds: i64,
    local_end_nanoseconds: i64,
    time_zone: String,
    start_seconds: i64,
    start_nanoseconds: i64,
    end_seconds: i64,
    end_nanoseconds: i64,
    resolution: String,
    view_scope: String,
}

fn raw_pending_event_creation_from_row(
    row: &async_sqlite::rusqlite::Row<'_>,
) -> async_sqlite::rusqlite::Result<RawPendingEventCreation> {
    Ok(RawPendingEventCreation {
        range_start_seconds: row.get(0)?,
        range_start_nanoseconds: row.get(1)?,
        range_end_seconds: row.get(2)?,
        range_end_nanoseconds: row.get(3)?,
        local_start_julian_day: row.get(4)?,
        local_end_julian_day: row.get(5)?,
        creation_id: row.get(6)?,
        subject: row.get(7)?,
        local_start_seconds: row.get(8)?,
        local_start_nanoseconds: row.get(9)?,
        local_end_seconds: row.get(10)?,
        local_end_nanoseconds: row.get(11)?,
        time_zone: row.get(12)?,
        start_seconds: row.get(13)?,
        start_nanoseconds: row.get(14)?,
        end_seconds: row.get(15)?,
        end_nanoseconds: row.get(16)?,
        resolution: row.get(17)?,
        view_scope: row.get(18)?,
    })
}

pub(super) fn read_pending_event_creation(
    connection: &Connection,
    account_id: &str,
) -> CacheResult<Option<PendingCalendarEventCreation>> {
    let raw = connection
        .query_row(
            "SELECT range_start_seconds, range_start_nanoseconds,
                    range_end_seconds, range_end_nanoseconds,
                    local_start_julian_day, local_end_julian_day,
                    creation_id, subject,
                    local_start_seconds, local_start_nanoseconds,
                    local_end_seconds, local_end_nanoseconds,
                    time_zone, start_seconds, start_nanoseconds,
                    end_seconds, end_nanoseconds, resolution, view_scope
             FROM calendar_pending_event_creation
             WHERE account_id = ?1",
            [account_id],
            raw_pending_event_creation_from_row,
        )
        .optional()?;
    raw.map(|raw| decode_pending_event_creation(raw, account_id))
        .transpose()
}

pub(super) fn mark_pending_event_creation_rejected(
    transaction: &Transaction<'_>,
    creation: &CalendarEventCreation,
) -> CalendarCacheResult<()> {
    let account_id = creation.view().account_id().as_str();
    match read_pending_event_creation(transaction, account_id)? {
        None => Ok(()),
        Some(pending) if pending.creation() == creation => {
            let updated = transaction.execute(
                "UPDATE calendar_pending_event_creation
                 SET resolution = 'rejected'
                 WHERE account_id = ?1 AND creation_id = ?2",
                params![account_id, creation.id().as_uuid().to_string()],
            )?;
            if updated == 1 {
                Ok(())
            } else {
                Err(CacheFailure::InvalidData.into())
            }
        }
        Some(_) => Err(CalendarCacheFailure::CreationAlreadyPending),
    }
}

fn decode_pending_event_creation(
    raw: RawPendingEventCreation,
    account_id: &str,
) -> CacheResult<PendingCalendarEventCreation> {
    let account_id =
        AccountId::try_from(account_id.to_owned()).map_err(|_| CacheFailure::InvalidData)?;
    let start = offset_datetime_from_parts(raw.range_start_seconds, raw.range_start_nanoseconds)?;
    let end = offset_datetime_from_parts(raw.range_end_seconds, raw.range_end_nanoseconds)?;
    let start_date =
        Date::from_julian_day(raw.local_start_julian_day).map_err(|_| CacheFailure::InvalidData)?;
    let end_date =
        Date::from_julian_day(raw.local_end_julian_day).map_err(|_| CacheFailure::InvalidData)?;
    let range = CalendarViewRange::new(start, end, start_date, end_date)
        .map_err(|_| CacheFailure::InvalidData)?;
    let view = CalendarViewReference::new(account_id, range);
    let local_start =
        primitive_datetime_from_parts(raw.local_start_seconds, raw.local_start_nanoseconds)?;
    let local_end =
        primitive_datetime_from_parts(raw.local_end_seconds, raw.local_end_nanoseconds)?;
    let time_zone =
        CalendarTimeZoneId::try_from(raw.time_zone).map_err(|_| CacheFailure::InvalidData)?;
    let start = offset_datetime_from_parts(raw.start_seconds, raw.start_nanoseconds)?;
    let end = offset_datetime_from_parts(raw.end_seconds, raw.end_nanoseconds)?;
    let draft =
        CalendarEventDraft::timed(raw.subject, local_start, local_end, time_zone, start, end)
            .map_err(|_| CacheFailure::InvalidData)?;
    let id =
        CalendarEventCreationId::parse(&raw.creation_id).map_err(|_| CacheFailure::InvalidData)?;
    let creation = match raw.view_scope.as_str() {
        "bounded" => CalendarEventCreation::try_restore(id, view, draft),
        "legacy_unbounded" => Ok(CalendarEventCreation::restore_legacy_pending(
            id, view, draft,
        )),
        _ => return Err(CacheFailure::InvalidData),
    }
    .map_err(|_| CacheFailure::InvalidData)?;
    match raw.resolution.as_str() {
        "unresolved" => Ok(PendingCalendarEventCreation::Unresolved(creation)),
        "rejected" => Ok(PendingCalendarEventCreation::Rejected(creation)),
        _ => Err(CacheFailure::InvalidData),
    }
}

pub(super) fn write_pending_event_creation(
    connection: &Connection,
    creation: &CalendarEventCreation,
) -> CacheResult<()> {
    if creation.is_legacy_pending() {
        return Err(CacheFailure::InvalidData);
    }
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        return Err(CacheFailure::InvalidData);
    };
    if !creation.draft().time().overlaps(creation.view().range()) {
        return Err(CacheFailure::InvalidData);
    }
    let scope = CalendarScope::from_view(creation.view());
    let (local_start_seconds, local_start_nanoseconds) =
        primitive_datetime_parts(creation.draft().local_start());
    let (local_end_seconds, local_end_nanoseconds) =
        primitive_datetime_parts(creation.draft().local_end());
    let (start_seconds, start_nanoseconds) = offset_datetime_parts(*start);
    let (end_seconds, end_nanoseconds) = offset_datetime_parts(*end);
    connection.execute(
        "INSERT INTO calendar_pending_event_creation (
            account_id,
            range_start_seconds,
            range_start_nanoseconds,
            range_end_seconds,
            range_end_nanoseconds,
            local_start_julian_day,
            local_end_julian_day,
            creation_id,
            subject,
            local_start_seconds,
            local_start_nanoseconds,
            local_end_seconds,
            local_end_nanoseconds,
            time_zone,
            start_seconds,
            start_nanoseconds,
            end_seconds,
            end_nanoseconds,
            view_scope
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
            ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, 'bounded'
         )",
        params![
            creation.view().account_id().as_str(),
            scope.start_seconds,
            scope.start_nanoseconds,
            scope.end_seconds,
            scope.end_nanoseconds,
            scope.local_start_julian_day,
            scope.local_end_julian_day,
            creation.id().as_uuid().to_string(),
            creation.draft().subject(),
            local_start_seconds,
            local_start_nanoseconds,
            local_end_seconds,
            local_end_nanoseconds,
            creation.draft().time_zone().as_str(),
            start_seconds,
            start_nanoseconds,
            end_seconds,
            end_nanoseconds
        ],
    )?;
    Ok(())
}

fn primitive_datetime_parts(value: PrimitiveDateTime) -> (i64, i64) {
    offset_datetime_parts(value.assume_utc())
}

fn primitive_datetime_from_parts(seconds: i64, nanoseconds: i64) -> CacheResult<PrimitiveDateTime> {
    let value = offset_datetime_from_parts(seconds, nanoseconds)?;
    Ok(PrimitiveDateTime::new(value.date(), value.time()))
}

pub(super) fn confirms_calendar_event_creation(
    creation: &CalendarEventCreation,
    event: &CalendarOccurrence,
) -> bool {
    event.reference() == creation.view()
        && event.subject() == Some(creation.draft().subject())
        && event.time() == creation.draft().time()
        && event.overlaps_view()
}

pub(super) fn delete_calendar_event_from(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event_id: &str,
    table: CalendarEventTable,
) -> CacheResult<()> {
    let table_name = match table {
        CalendarEventTable::Active => "calendar_events",
        CalendarEventTable::Rebuild => "calendar_event_rebuild",
    };
    let query = format!(
        "DELETE FROM {table_name}
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7
           AND event_id = ?8"
    );
    connection.execute(
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
    )?;
    Ok(())
}

pub(super) fn commit_calendar_event_update(
    transaction: &Transaction<'_>,
    account_id: &str,
    source_range: CalendarScope,
    event: &CalendarOccurrence,
) -> CacheResult<()> {
    require_cached_source_view(transaction, account_id, source_range)?;
    let scopes = cached_calendar_scopes(transaction, account_id)?;
    transaction.execute(
        "DELETE FROM calendar_events WHERE account_id = ?1 AND event_id = ?2",
        params![account_id, event.id().as_str()],
    )?;
    write_event_to_cached_scopes(transaction, account_id, scopes, event)?;
    clear_calendar_write_sync_required(transaction, account_id)
}

pub(super) fn commit_calendar_event_deletion(
    transaction: &Transaction<'_>,
    account_id: &str,
    source_range: CalendarScope,
    event: &VersionedCalendarEvent,
) -> CacheResult<()> {
    require_cached_source_view(transaction, account_id, source_range)?;
    transaction.execute(
        "DELETE FROM calendar_events WHERE account_id = ?1 AND event_id = ?2",
        params![account_id, event.occurrence().id().as_str(),],
    )?;
    clear_calendar_write_sync_required(transaction, account_id)
}

fn require_cached_source_view(
    transaction: &Transaction<'_>,
    account_id: &str,
    source_range: CalendarScope,
) -> CacheResult<()> {
    let source_view_exists = transaction
        .query_row(
            "SELECT 1 FROM calendar_view_sync_state
             WHERE account_id = ?1
               AND range_start_seconds = ?2
               AND range_start_nanoseconds = ?3
               AND range_end_seconds = ?4
               AND range_end_nanoseconds = ?5
               AND local_start_julian_day = ?6
               AND local_end_julian_day = ?7",
            params![
                account_id,
                source_range.start_seconds,
                source_range.start_nanoseconds,
                source_range.end_seconds,
                source_range.end_nanoseconds,
                source_range.local_start_julian_day,
                source_range.local_end_julian_day,
            ],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if source_view_exists {
        Ok(())
    } else {
        Err(CacheFailure::InvalidData)
    }
}

fn cached_calendar_scopes(
    transaction: &Transaction<'_>,
    account_id: &str,
) -> CacheResult<Vec<CalendarScope>> {
    let mut statement = transaction.prepare(
        "SELECT range_start_seconds, range_start_nanoseconds,
                range_end_seconds, range_end_nanoseconds,
                local_start_julian_day, local_end_julian_day
         FROM calendar_view_sync_state
         WHERE account_id = ?1",
    )?;
    statement
        .query_map([account_id], calendar_scope_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn calendar_scope_from_row(
    row: &async_sqlite::rusqlite::Row<'_>,
) -> async_sqlite::rusqlite::Result<CalendarScope> {
    Ok(CalendarScope {
        start_seconds: row.get(0)?,
        start_nanoseconds: row.get(1)?,
        end_seconds: row.get(2)?,
        end_nanoseconds: row.get(3)?,
        local_start_julian_day: row.get(4)?,
        local_end_julian_day: row.get(5)?,
    })
}

fn write_event_to_cached_scopes(
    transaction: &Transaction<'_>,
    account_id: &str,
    scopes: Vec<CalendarScope>,
    event: &CalendarOccurrence,
) -> CacheResult<()> {
    let account_id =
        AccountId::try_from(account_id.to_owned()).map_err(|_| CacheFailure::InvalidData)?;
    for range in scopes {
        let view_range = range.view_range()?;
        if !event.time().overlaps(&view_range) {
            continue;
        }
        let view = CalendarViewReference::new(account_id.clone(), view_range);
        write_calendar_event(
            transaction,
            CalendarEventScope { view: &view, range },
            event,
        )?;
    }
    Ok(())
}

pub(super) fn mark_calendar_rebuild_removed(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event_id: &str,
) -> CacheResult<()> {
    connection.execute(
        "INSERT OR IGNORE INTO calendar_event_rebuild_removed (
            account_id,
            range_start_seconds,
            range_start_nanoseconds,
            range_end_seconds,
            range_end_nanoseconds,
            local_start_julian_day,
            local_end_julian_day,
            event_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
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
    )?;
    Ok(())
}

pub(super) fn clear_calendar_rebuild_removed(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event_id: &str,
) -> CacheResult<()> {
    connection.execute(
        "DELETE FROM calendar_event_rebuild_removed
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7
           AND event_id = ?8",
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
    )?;
    Ok(())
}

pub(super) fn calendar_rebuild_event_was_removed(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event_id: &str,
) -> CacheResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM calendar_event_rebuild_removed
                WHERE account_id = ?1
                  AND range_start_seconds = ?2
                  AND range_start_nanoseconds = ?3
                  AND range_end_seconds = ?4
                  AND range_end_nanoseconds = ?5
                  AND local_start_julian_day = ?6
                  AND local_end_julian_day = ?7
                  AND event_id = ?8
            )",
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
            |row| row.get(0),
        )
        .map_err(CacheFailure::from)
}
