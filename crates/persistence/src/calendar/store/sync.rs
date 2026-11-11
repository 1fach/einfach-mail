use super::*;

pub(super) fn commit_calendar_page(
    connection: &mut Connection,
    commit: CalendarPageCommit,
) -> CacheResult<CalendarSyncState> {
    let scope = CalendarEventScope {
        view: &commit.view,
        range: commit.scope,
    };
    let account_id = scope.account_id().to_owned();
    let transaction = connection.transaction()?;
    insert_calendar_view_state(&transaction, &account_id, commit.scope)?;
    let replacing =
        read_calendar_sync_state(&transaction, &account_id, commit.scope)?.replacement_active();
    let finish_replacement =
        replacing && matches!(&commit.progress, CalendarDeltaProgress::Complete(_));
    apply_calendar_changes(
        &transaction,
        scope,
        commit.changes,
        if replacing {
            CalendarEventTable::Rebuild
        } else {
            CalendarEventTable::Active
        },
    )?;
    if finish_replacement {
        replace_calendar_snapshot(&transaction, scope)?;
    }
    clear_calendar_snapshot_state(&transaction, &account_id, commit.scope)?;
    persist_calendar_progress(
        &transaction,
        CalendarProgressCommit {
            account_id: account_id.clone(),
            scope: commit.scope,
            progress: commit.progress,
            completed_at: commit.completed_at,
            finish_replacement,
        },
    )?;
    let state = read_calendar_sync_state(&transaction, &account_id, commit.scope)?;
    transaction.commit()?;
    Ok(state)
}

fn apply_calendar_changes(
    transaction: &async_sqlite::rusqlite::Transaction<'_>,
    scope: CalendarEventScope<'_>,
    changes: Vec<CalendarDeltaChange>,
    table: CalendarEventTable,
) -> CacheResult<()> {
    let context = CalendarEventChangeContext {
        transaction,
        scope,
        table,
    };
    for change in changes {
        context.apply(change)?;
    }
    Ok(())
}

struct CalendarEventChangeContext<'scope, 'connection> {
    transaction: &'scope async_sqlite::rusqlite::Transaction<'connection>,
    scope: CalendarEventScope<'scope>,
    table: CalendarEventTable,
}

impl CalendarEventChangeContext<'_, '_> {
    fn apply(&self, change: CalendarDeltaChange) -> CacheResult<()> {
        match change {
            CalendarDeltaChange::Removed { id } => {
                delete_calendar_event_from(self.transaction, self.scope, id.as_str(), self.table)?;
                match self.table {
                    CalendarEventTable::Active => Ok(()),
                    CalendarEventTable::Rebuild => {
                        mark_calendar_rebuild_removed(self.transaction, self.scope, id.as_str())
                    }
                }
            }
            CalendarDeltaChange::Upsert { id, patch } => self.apply_upsert(id, patch),
        }
    }

    fn apply_upsert(
        &self,
        id: einfach_mail_domain::EventId,
        patch: CalendarOccurrencePatch,
    ) -> CacheResult<()> {
        let occurrence = self.occurrence_after_patch(&id, patch)?;
        match self.table {
            CalendarEventTable::Active if occurrence.overlaps_view() => {
                write_calendar_event(self.transaction, self.scope, &occurrence)
            }
            CalendarEventTable::Active => {
                delete_calendar_event_from(self.transaction, self.scope, id.as_str(), self.table)
            }
            CalendarEventTable::Rebuild => {
                clear_calendar_rebuild_removed(self.transaction, self.scope, id.as_str())?;
                write_calendar_rebuild_event(self.transaction, self.scope, &occurrence)
            }
        }
    }

    fn occurrence_after_patch(
        &self,
        id: &einfach_mail_domain::EventId,
        patch: CalendarOccurrencePatch,
    ) -> CacheResult<CalendarOccurrence> {
        let removed_from_rebuild = self.table == CalendarEventTable::Rebuild
            && calendar_rebuild_event_was_removed(self.transaction, self.scope, id.as_str())?;
        let existing = match read_calendar_event_from(
            self.transaction,
            self.scope,
            id.as_str(),
            self.table,
        )? {
            Some(occurrence) => Some(occurrence),
            None if self.table == CalendarEventTable::Rebuild && !removed_from_rebuild => {
                read_calendar_event(self.transaction, self.scope, id.as_str())?
            }
            None => None,
        };
        match existing {
            Some(mut occurrence) => {
                patch
                    .apply_to(&mut occurrence)
                    .map_err(|_| CacheFailure::InvalidData)?;
                Ok(occurrence)
            }
            None => patch
                .into_new(self.scope.view.clone(), id.clone())
                .map_err(|_| CacheFailure::InvalidData),
        }
    }
}

fn persist_calendar_progress(
    transaction: &async_sqlite::rusqlite::Transaction<'_>,
    update: CalendarProgressCommit,
) -> CacheResult<()> {
    let (delta_link, next_link, last_sync) = match update.progress {
        CalendarDeltaProgress::Next(link) => (None, Some(link), None),
        CalendarDeltaProgress::Complete(link) => (Some(link), None, Some(update.completed_at)),
    };
    transaction.execute(
        UPDATE_CALENDAR_SYNC_PROGRESS,
        params![
            update.account_id,
            update.scope.start_seconds,
            update.scope.start_nanoseconds,
            update.scope.end_seconds,
            update.scope.end_nanoseconds,
            update.scope.local_start_julian_day,
            update.scope.local_end_julian_day,
            delta_link.map(|link| link.as_str().to_owned()),
            next_link.map(|link| link.as_str().to_owned()),
            last_sync.map(|(seconds, _)| seconds),
            last_sync.map(|(_, nanoseconds)| nanoseconds),
            i64::from(update.finish_replacement)
        ],
    )?;
    Ok(())
}

pub(super) fn insert_calendar_view_state(
    connection: &Connection,
    account_id: &str,
    scope: CalendarScope,
) -> CacheResult<()> {
    connection.execute(
        "INSERT OR IGNORE INTO calendar_view_sync_state (
            account_id,
            range_start_seconds,
            range_start_nanoseconds,
            range_end_seconds,
            range_end_nanoseconds,
            local_start_julian_day,
            local_end_julian_day
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            account_id,
            scope.start_seconds,
            scope.start_nanoseconds,
            scope.end_seconds,
            scope.end_nanoseconds,
            scope.local_start_julian_day,
            scope.local_end_julian_day
        ],
    )?;
    Ok(())
}

pub(super) fn read_calendar_sync_state(
    connection: &Connection,
    account_id: &str,
    scope: CalendarScope,
) -> CacheResult<CalendarSyncState> {
    let raw = connection
        .query_row(
            SELECT_CALENDAR_SYNC_STATE_SQL,
            params![
                account_id,
                scope.start_seconds,
                scope.start_nanoseconds,
                scope.end_seconds,
                scope.end_nanoseconds,
                scope.local_start_julian_day,
                scope.local_end_julian_day
            ],
            |row| {
                Ok(RawCalendarSyncState {
                    delta_link: row.get(0)?,
                    next_link: row.get(1)?,
                    last_sync_seconds: row.get(2)?,
                    last_sync_nanoseconds: row.get(3)?,
                    replacement_active: row.get(4)?,
                    snapshot_seconds: row.get(5)?,
                    snapshot_nanoseconds: row.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or(CacheFailure::InvalidData)?;
    calendar_sync_state_from_raw(raw)
}

fn calendar_sync_state_from_raw(raw: RawCalendarSyncState) -> CacheResult<CalendarSyncState> {
    let delta_completed_at =
        optional_calendar_completion(raw.last_sync_seconds, raw.last_sync_nanoseconds)?;
    let snapshot_completed_at =
        optional_calendar_completion(raw.snapshot_seconds, raw.snapshot_nanoseconds)?;
    let (mode, last_synchronized_at) =
        calendar_sync_mode(&raw, delta_completed_at, snapshot_completed_at)?;
    CalendarSyncState::from_checkpoint(
        raw.delta_link.map(CalendarDeltaLink::from_opaque),
        raw.next_link.map(CalendarDeltaLink::from_opaque),
        last_synchronized_at,
        mode,
    )
    .map_err(|_| CacheFailure::InvalidData)
}

fn optional_calendar_completion(
    seconds: Option<i64>,
    nanoseconds: Option<i64>,
) -> CacheResult<Option<SystemTime>> {
    Ok(match (seconds, nanoseconds) {
        (None, None) => None,
        (Some(seconds), Some(nanoseconds)) => Some(system_time_from_parts(seconds, nanoseconds)?),
        _ => return Err(CacheFailure::InvalidData),
    })
}

fn calendar_sync_mode(
    raw: &RawCalendarSyncState,
    delta_completed_at: Option<SystemTime>,
    snapshot_completed_at: Option<SystemTime>,
) -> CacheResult<(CalendarSyncMode, Option<SystemTime>)> {
    let has_delta_state = raw.delta_link.is_some()
        || raw.next_link.is_some()
        || delta_completed_at.is_some()
        || raw.replacement_active != 0;
    match (raw.replacement_active, snapshot_completed_at) {
        (0, Some(completed_at)) if !has_delta_state => {
            Ok((CalendarSyncMode::Snapshot, Some(completed_at)))
        }
        (0, None) => Ok((CalendarSyncMode::Incremental, delta_completed_at)),
        (1, None) => Ok((CalendarSyncMode::Replacement, delta_completed_at)),
        _ => Err(CacheFailure::InvalidData),
    }
}

struct RawCalendarSyncState {
    delta_link: Option<String>,
    next_link: Option<String>,
    last_sync_seconds: Option<i64>,
    last_sync_nanoseconds: Option<i64>,
    replacement_active: i64,
    snapshot_seconds: Option<i64>,
    snapshot_nanoseconds: Option<i64>,
}

const UPDATE_CALENDAR_SYNC_PROGRESS: &str = "UPDATE calendar_view_sync_state
    SET delta_link = COALESCE(?8, delta_link),
        next_link = ?9,
        last_sync_seconds = COALESCE(?10, last_sync_seconds),
        last_sync_nanoseconds = COALESCE(?11, last_sync_nanoseconds),
        write_sync_required = CASE WHEN ?10 IS NOT NULL THEN 0 ELSE write_sync_required END,
        replacement_active = CASE WHEN ?12 = 1 THEN 0 ELSE replacement_active END
    WHERE account_id = ?1
      AND range_start_seconds = ?2
      AND range_start_nanoseconds = ?3
      AND range_end_seconds = ?4
      AND range_end_nanoseconds = ?5
      AND local_start_julian_day = ?6
      AND local_end_julian_day = ?7";

const SELECT_CALENDAR_SYNC_STATE_SQL: &str = "SELECT state.delta_link, state.next_link,
    state.last_sync_seconds, state.last_sync_nanoseconds, state.replacement_active,
    snapshot.completed_seconds, snapshot.completed_nanoseconds
FROM calendar_view_sync_state AS state
LEFT JOIN calendar_view_snapshot_state AS snapshot
    ON snapshot.account_id = state.account_id
   AND snapshot.range_start_seconds = state.range_start_seconds
   AND snapshot.range_start_nanoseconds = state.range_start_nanoseconds
   AND snapshot.range_end_seconds = state.range_end_seconds
   AND snapshot.range_end_nanoseconds = state.range_end_nanoseconds
   AND snapshot.local_start_julian_day = state.local_start_julian_day
   AND snapshot.local_end_julian_day = state.local_end_julian_day
WHERE state.account_id = ?1
  AND state.range_start_seconds = ?2
  AND state.range_start_nanoseconds = ?3
  AND state.range_end_seconds = ?4
  AND state.range_end_nanoseconds = ?5
  AND state.local_start_julian_day = ?6
  AND state.local_end_julian_day = ?7";
