use super::*;

pub(super) fn clear_calendar_snapshot_state(
    transaction: &Transaction<'_>,
    account_id: &str,
    scope: CalendarScope,
) -> CacheResult<()> {
    delete_calendar_rows_for_scope(
        transaction,
        "calendar_view_snapshot_state",
        account_id,
        scope,
    )
}

pub(super) fn commit_calendar_snapshot(
    connection: &mut Connection,
    commit: CalendarSnapshotCommit,
) -> CacheResult<CalendarSyncState> {
    let scope = CalendarEventScope {
        view: &commit.view,
        range: commit.scope,
    };
    let account_id = scope.account_id().to_owned();
    let transaction = connection.transaction()?;
    insert_calendar_view_state(&transaction, &account_id, commit.scope)?;
    clear_calendar_snapshot_rows(&transaction, &account_id, commit.scope)?;
    for event in &commit.events {
        write_calendar_event(&transaction, scope, event)?;
    }
    persist_calendar_snapshot_state(&transaction, &account_id, commit.scope, commit.completed_at)?;
    let state = read_calendar_sync_state(&transaction, &account_id, commit.scope)?;
    transaction.commit()?;
    Ok(state)
}

fn clear_calendar_snapshot_rows(
    transaction: &Transaction<'_>,
    account_id: &str,
    scope: CalendarScope,
) -> CacheResult<()> {
    for table in [
        "calendar_event_rebuild_removed",
        "calendar_event_rebuild",
        "calendar_events",
    ] {
        delete_calendar_rows_for_scope(transaction, table, account_id, scope)?;
    }
    Ok(())
}

fn delete_calendar_rows_for_scope(
    transaction: &Transaction<'_>,
    table: &str,
    account_id: &str,
    scope: CalendarScope,
) -> CacheResult<()> {
    let statement = format!("DELETE FROM {table}{CALENDAR_SCOPE_FILTER_SQL}");
    transaction.execute(
        &statement,
        params_from_iter(calendar_scope_params(account_id, scope)),
    )?;
    Ok(())
}

fn persist_calendar_snapshot_state(
    transaction: &Transaction<'_>,
    account_id: &str,
    scope: CalendarScope,
    completed_at: (i64, i64),
) -> CacheResult<()> {
    clear_calendar_delta_sync_state(transaction, account_id, scope)?;
    transaction.execute(
        UPSERT_CALENDAR_SNAPSHOT_STATE_SQL,
        params_from_iter(calendar_snapshot_state_params(
            account_id,
            scope,
            completed_at,
        )),
    )?;
    Ok(())
}

fn clear_calendar_delta_sync_state(
    transaction: &Transaction<'_>,
    account_id: &str,
    scope: CalendarScope,
) -> CacheResult<()> {
    let changed = transaction.execute(
        CLEAR_CALENDAR_DELTA_SYNC_STATE_SQL,
        params_from_iter(calendar_scope_params(account_id, scope)),
    )?;
    if changed != 1 {
        return Err(CacheFailure::InvalidData);
    }
    Ok(())
}

fn calendar_snapshot_state_params(
    account_id: &str,
    scope: CalendarScope,
    completed_at: (i64, i64),
) -> [async_sqlite::rusqlite::types::Value; 9] {
    [
        account_id.to_owned().into(),
        scope.start_seconds.into(),
        scope.start_nanoseconds.into(),
        scope.end_seconds.into(),
        scope.end_nanoseconds.into(),
        i64::from(scope.local_start_julian_day).into(),
        i64::from(scope.local_end_julian_day).into(),
        completed_at.0.into(),
        completed_at.1.into(),
    ]
}

pub(super) fn begin_calendar_full_resync(
    connection: &mut Connection,
    account_id: &str,
    scope: CalendarScope,
    reset_link: Option<String>,
) -> CacheResult<CalendarSyncState> {
    let transaction = connection.transaction()?;
    insert_calendar_view_state(&transaction, account_id, scope)?;
    transaction.execute(
        "DELETE FROM calendar_event_rebuild
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7",
        params_from_iter(calendar_scope_params(account_id, scope)),
    )?;
    transaction.execute(
        "DELETE FROM calendar_event_rebuild_removed
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7",
        params_from_iter(calendar_scope_params(account_id, scope)),
    )?;
    let changed = transaction.execute(
        "UPDATE calendar_view_sync_state
         SET next_link = ?8, replacement_active = 1
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7",
        params![
            account_id,
            scope.start_seconds,
            scope.start_nanoseconds,
            scope.end_seconds,
            scope.end_nanoseconds,
            scope.local_start_julian_day,
            scope.local_end_julian_day,
            reset_link
        ],
    )?;
    if changed != 1 {
        return Err(CacheFailure::InvalidData);
    }
    let state = read_calendar_sync_state(&transaction, account_id, scope)?;
    transaction.commit()?;
    Ok(state)
}

fn calendar_scope_params(
    account_id: &str,
    scope: CalendarScope,
) -> [async_sqlite::rusqlite::types::Value; 7] {
    [
        account_id.to_owned().into(),
        scope.start_seconds.into(),
        scope.start_nanoseconds.into(),
        scope.end_seconds.into(),
        scope.end_nanoseconds.into(),
        i64::from(scope.local_start_julian_day).into(),
        i64::from(scope.local_end_julian_day).into(),
    ]
}

pub(super) fn replace_calendar_snapshot(
    transaction: &async_sqlite::rusqlite::Transaction<'_>,
    scope: CalendarEventScope<'_>,
) -> CacheResult<()> {
    let scope_params = || calendar_scope_params(scope.account_id(), scope.range);
    transaction.execute(
        "DELETE FROM calendar_events
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7",
        params_from_iter(scope_params()),
    )?;
    transaction.execute(
        "INSERT INTO calendar_events (
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
         )
         SELECT
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
         FROM calendar_event_rebuild
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7
           AND is_in_view = 1",
        params_from_iter(scope_params()),
    )?;
    transaction.execute(
        "DELETE FROM calendar_event_rebuild
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7",
        params_from_iter(scope_params()),
    )?;
    transaction.execute(
        "DELETE FROM calendar_event_rebuild_removed
         WHERE account_id = ?1
           AND range_start_seconds = ?2
           AND range_start_nanoseconds = ?3
           AND range_end_seconds = ?4
           AND range_end_nanoseconds = ?5
           AND local_start_julian_day = ?6
           AND local_end_julian_day = ?7",
        params_from_iter(scope_params()),
    )?;
    Ok(())
}

const CALENDAR_SCOPE_FILTER_SQL: &str = "
    WHERE account_id = ?1
      AND range_start_seconds = ?2
      AND range_start_nanoseconds = ?3
      AND range_end_seconds = ?4
      AND range_end_nanoseconds = ?5
      AND local_start_julian_day = ?6
      AND local_end_julian_day = ?7";

const CLEAR_CALENDAR_DELTA_SYNC_STATE_SQL: &str = "UPDATE calendar_view_sync_state
    SET delta_link = NULL,
        next_link = NULL,
        last_sync_seconds = NULL,
        last_sync_nanoseconds = NULL,
        replacement_active = 0,
        write_sync_required = 0
    WHERE account_id = ?1
      AND range_start_seconds = ?2
      AND range_start_nanoseconds = ?3
      AND range_end_seconds = ?4
      AND range_end_nanoseconds = ?5
      AND local_start_julian_day = ?6
      AND local_end_julian_day = ?7";

const UPSERT_CALENDAR_SNAPSHOT_STATE_SQL: &str = "INSERT INTO calendar_view_snapshot_state (
    account_id,
    range_start_seconds,
    range_start_nanoseconds,
    range_end_seconds,
    range_end_nanoseconds,
    local_start_julian_day,
    local_end_julian_day,
    completed_seconds,
    completed_nanoseconds
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT (
    account_id,
    range_start_seconds,
    range_start_nanoseconds,
    range_end_seconds,
    range_end_nanoseconds,
    local_start_julian_day,
    local_end_julian_day
) DO UPDATE SET
    completed_seconds = excluded.completed_seconds,
    completed_nanoseconds = excluded.completed_nanoseconds";
