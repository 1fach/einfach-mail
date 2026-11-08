use std::{future::Future, time::SystemTime};

use async_sqlite::rusqlite::{
    Connection, OptionalExtension, Transaction, params, params_from_iter,
};
use einfach_mail_application::{
    CalendarCache, CalendarCacheError, CalendarSyncMode, CalendarSyncState,
};
use einfach_mail_domain::{
    AccountId, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarEventCreation, CalendarEventCreationId, CalendarEventDraft, CalendarEventUpdate,
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, CalendarTimeZoneId, CalendarViewRange, CalendarViewReference,
    EditableCalendarEvent, PendingCalendarEventCreation,
};
use thiserror::Error;
use time::{Date, OffsetDateTime, PrimitiveDateTime};

use super::{
    CacheFailure, CacheResult, SqliteAccountCache,
    system_time::{system_time_from_parts, system_time_parts},
};

type CalendarCacheResult<T> = Result<T, CalendarCacheFailure>;

#[derive(Debug, Error)]
enum CalendarCacheFailure {
    #[error(transparent)]
    Store(#[from] CacheFailure),
    #[error("another calendar event creation is already pending")]
    CreationAlreadyPending,
}

impl CalendarCacheFailure {
    fn public(self) -> CalendarCacheError {
        match self {
            Self::Store(error) => error.calendar_public(),
            Self::CreationAlreadyPending => CalendarCacheError::CreationAlreadyPending,
        }
    }
}

impl From<async_sqlite::Error> for CalendarCacheFailure {
    fn from(error: async_sqlite::Error) -> Self {
        Self::Store(CacheFailure::from(error))
    }
}

impl From<async_sqlite::rusqlite::Error> for CalendarCacheFailure {
    fn from(error: async_sqlite::rusqlite::Error) -> Self {
        Self::Store(CacheFailure::from(error))
    }
}

#[derive(Clone, Copy)]
struct CalendarScope {
    start_seconds: i64,
    start_nanoseconds: i64,
    end_seconds: i64,
    end_nanoseconds: i64,
    local_start_julian_day: i32,
    local_end_julian_day: i32,
}

#[derive(Clone, Copy)]
struct CalendarEventScope<'a> {
    view: &'a CalendarViewReference,
    range: CalendarScope,
}

impl<'a> CalendarEventScope<'a> {
    fn account_id(self) -> &'a str {
        self.view.account_id().as_str()
    }
}

struct CalendarPageCommit {
    view: CalendarViewReference,
    scope: CalendarScope,
    changes: Vec<CalendarDeltaChange>,
    progress: CalendarDeltaProgress,
    completed_at: (i64, i64),
}

struct CalendarProgressCommit {
    account_id: String,
    scope: CalendarScope,
    progress: CalendarDeltaProgress,
    completed_at: (i64, i64),
    finish_replacement: bool,
}

struct CalendarEventTimeColumns {
    is_all_day: i64,
    start_seconds: Option<i64>,
    start_nanoseconds: Option<i64>,
    end_seconds: Option<i64>,
    end_nanoseconds: Option<i64>,
    start_day: Option<i32>,
    end_day: Option<i32>,
}

impl CalendarEventTimeColumns {
    fn from_time(time: &CalendarOccurrenceTime) -> Self {
        match time {
            CalendarOccurrenceTime::Timed { start, end } => Self::from_instants(*start, *end),
            CalendarOccurrenceTime::AllDay {
                start,
                end_exclusive,
            } => Self::from_dates(*start, *end_exclusive),
        }
    }

    fn from_instants(start: OffsetDateTime, end: OffsetDateTime) -> Self {
        let (start_seconds, start_nanoseconds) = offset_datetime_parts(start);
        let (end_seconds, end_nanoseconds) = offset_datetime_parts(end);
        Self {
            is_all_day: 0,
            start_seconds: Some(start_seconds),
            start_nanoseconds: Some(start_nanoseconds),
            end_seconds: Some(end_seconds),
            end_nanoseconds: Some(end_nanoseconds),
            start_day: None,
            end_day: None,
        }
    }

    fn from_dates(start: Date, end_exclusive: Date) -> Self {
        Self {
            is_all_day: 1,
            start_seconds: None,
            start_nanoseconds: None,
            end_seconds: None,
            end_nanoseconds: None,
            start_day: Some(start.to_julian_day()),
            end_day: Some(end_exclusive.to_julian_day()),
        }
    }
}

impl CalendarScope {
    fn from_view(view: &CalendarViewReference) -> Self {
        let range = view.range();
        Self {
            start_seconds: range.start().unix_timestamp(),
            start_nanoseconds: i64::from(range.start().nanosecond()),
            end_seconds: range.end().unix_timestamp(),
            end_nanoseconds: i64::from(range.end().nanosecond()),
            local_start_julian_day: range.start_date().to_julian_day(),
            local_end_julian_day: range.end_date_exclusive().to_julian_day(),
        }
    }

    fn view_range(self) -> CacheResult<CalendarViewRange> {
        let start = offset_datetime_from_parts(self.start_seconds, self.start_nanoseconds)?;
        let end = offset_datetime_from_parts(self.end_seconds, self.end_nanoseconds)?;
        let start_date = Date::from_julian_day(self.local_start_julian_day)
            .map_err(|_| CacheFailure::InvalidData)?;
        let end_date = Date::from_julian_day(self.local_end_julian_day)
            .map_err(|_| CacheFailure::InvalidData)?;
        CalendarViewRange::new(start, end, start_date, end_date)
            .map_err(|_| CacheFailure::InvalidData)
    }
}

impl SqliteAccountCache {
    fn check_calendar_view(
        &self,
        view: &CalendarViewReference,
    ) -> Result<CalendarScope, CalendarCacheError> {
        if &self.account_id != view.account_id() {
            return Err(CalendarCacheError::AccountMismatch);
        }
        Ok(CalendarScope::from_view(view))
    }
}

impl CalendarCache for SqliteAccountCache {
    fn ensure_view(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(view);
        let account_id = view.account_id().as_str().to_owned();
        let client = self.client.clone();
        async move {
            let scope = validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    insert_calendar_view_state(&transaction, &account_id, scope)?;
                    let state = read_calendar_sync_state(&transaction, &account_id, scope)?;
                    transaction.commit()?;
                    Ok(state)
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn account_has_active_replacement(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<bool, CalendarCacheError>> + Send + '_ {
        let validation = self
            .check_account(account_id)
            .map_err(|_| CalendarCacheError::AccountMismatch);
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| {
                    calendar_account_has_active_replacement(connection, &account_id)
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn mark_calendar_write_sync_required(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(view);
        let account_id = view.account_id().as_str().to_owned();
        let client = self.client.clone();
        async move {
            let scope = validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    mark_calendar_write_sync_required(&transaction, &account_id, scope)?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn account_has_calendar_write_sync_required(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<bool, CalendarCacheError>> + Send + '_ {
        let validation = self
            .check_account(account_id)
            .map_err(|_| CalendarCacheError::AccountMismatch);
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| {
                    calendar_account_has_write_sync_required(connection, &account_id)
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn clear_calendar_write_sync_required(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let validation = self
            .check_account(account_id)
            .map_err(|_| CalendarCacheError::AccountMismatch);
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    clear_calendar_write_sync_required(&transaction, &account_id)?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn commit_delta_page(
        &self,
        view: &CalendarViewReference,
        page: CalendarDeltaPage,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(view);
        let completion = system_time_parts(completed_at).map_err(CacheFailure::calendar_public);
        let view = view.clone();
        let client = self.client.clone();
        async move {
            let scope = validation?;
            let completed_at = completion?;
            let (changes, progress) = page.into_parts();
            let commit = CalendarPageCommit {
                view,
                scope,
                changes,
                progress,
                completed_at,
            };
            client
                .conn_mut_and_then(move |connection| commit_calendar_page(connection, commit))
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn begin_full_resync(
        &self,
        view: &CalendarViewReference,
        reset_link: Option<CalendarDeltaLink>,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(view);
        let account_id = view.account_id().as_str().to_owned();
        let reset_link = reset_link.map(|link| link.as_str().to_owned());
        let client = self.client.clone();
        async move {
            let scope = validation?;
            client
                .conn_mut_and_then(move |connection| {
                    begin_calendar_full_resync(connection, &account_id, scope, reset_link)
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn list_events(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<Vec<CalendarOccurrence>, CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(view);
        let view = view.clone();
        let client = self.client.clone();
        async move {
            let scope = validation?;
            client
                .conn_and_then(move |connection| {
                    read_calendar_events(
                        connection,
                        CalendarEventScope {
                            view: &view,
                            range: scope,
                        },
                    )
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn stage_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(creation.view());
        let creation = creation.clone();
        let account_id = creation.view().account_id().as_str().to_owned();
        let client = self.client.clone();
        async move {
            let scope = validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    insert_calendar_view_state(&transaction, &account_id, scope)?;
                    match read_pending_event_creation(&transaction, &account_id)? {
                        Some(PendingCalendarEventCreation::Unresolved(existing))
                            if existing == creation => {}
                        Some(_) => return Err(CalendarCacheFailure::CreationAlreadyPending),
                        None => write_pending_event_creation(&transaction, &creation)?,
                    }
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CalendarCacheFailure::public)
        }
    }

    fn pending_event_creation(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Option<PendingCalendarEventCreation>, CalendarCacheError>> + Send + '_
    {
        let validation = if &self.account_id == account_id {
            Ok(())
        } else {
            Err(CalendarCacheError::AccountMismatch)
        };
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| {
                    read_pending_event_creation(connection, &account_id)
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn mark_event_creation_rejected(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(creation.view());
        let creation = creation.clone();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    mark_pending_event_creation_rejected(&transaction, &creation)?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CalendarCacheFailure::public)
        }
    }

    fn discard_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(creation.view());
        let creation = creation.clone();
        let account_id = creation.view().account_id().as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    match read_pending_event_creation(&transaction, &account_id)? {
                        None => {}
                        Some(PendingCalendarEventCreation::Rejected(pending))
                            if pending == creation =>
                        {
                            let deleted = transaction.execute(
                                "DELETE FROM calendar_pending_event_creation
                                 WHERE account_id = ?1 AND creation_id = ?2",
                                params![account_id, creation.id().as_uuid().to_string()],
                            )?;
                            if deleted != 1 {
                                return Err(CacheFailure::InvalidData.into());
                            }
                        }
                        Some(_) => return Err(CalendarCacheFailure::CreationAlreadyPending),
                    }
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CalendarCacheFailure::public)
        }
    }

    fn commit_event_creation(
        &self,
        creation: &CalendarEventCreation,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(creation.view());
        let creation = creation.clone();
        let event = event.clone();
        let client = self.client.clone();
        async move {
            let scope = validation?;
            if !confirms_calendar_event_creation(&creation, &event) {
                return Err(CalendarCacheError::InvalidData);
            }
            let account_id = creation.view().account_id().as_str().to_owned();
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    let pending = read_pending_event_creation(&transaction, &account_id)?;
                    if pending != Some(PendingCalendarEventCreation::Unresolved(creation.clone())) {
                        return Err(CacheFailure::InvalidData);
                    }
                    write_calendar_event(
                        &transaction,
                        CalendarEventScope {
                            view: creation.view(),
                            range: scope,
                        },
                        &event,
                    )?;
                    let deleted = transaction.execute(
                        "DELETE FROM calendar_pending_event_creation
                         WHERE account_id = ?1 AND creation_id = ?2",
                        params![account_id, creation.id().as_uuid().to_string()],
                    )?;
                    if deleted != 1 {
                        return Err(CacheFailure::InvalidData);
                    }
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn commit_event_update(
        &self,
        update: &CalendarEventUpdate,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(update.source().occurrence().reference());
        let update = update.clone();
        let event = event.clone();
        let client = self.client.clone();
        async move {
            let scope = validation?;
            if !update.confirms(&event) {
                return Err(CalendarCacheError::InvalidData);
            }
            let account_id = update
                .source()
                .occurrence()
                .reference()
                .account_id()
                .as_str()
                .to_owned();
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    commit_calendar_event_update(&transaction, &account_id, scope, &event)?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }

    fn commit_event_deletion(
        &self,
        event: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let validation = self.check_calendar_view(event.occurrence().reference());
        let event = event.clone();
        let client = self.client.clone();
        async move {
            let scope = validation?;
            let account_id = event
                .occurrence()
                .reference()
                .account_id()
                .as_str()
                .to_owned();
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    commit_calendar_event_deletion(&transaction, &account_id, scope, &event)?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::calendar_public)
        }
    }
}

fn calendar_account_has_active_replacement(
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

fn calendar_account_has_write_sync_required(
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

fn mark_calendar_write_sync_required(
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

fn clear_calendar_write_sync_required(
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

fn commit_calendar_page(
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

fn begin_calendar_full_resync(
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

fn replace_calendar_snapshot(
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

#[derive(Clone, Copy, Eq, PartialEq)]
enum CalendarEventTable {
    Active,
    Rebuild,
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

fn insert_calendar_view_state(
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

fn read_calendar_sync_state(
    connection: &Connection,
    account_id: &str,
    scope: CalendarScope,
) -> CacheResult<CalendarSyncState> {
    let raw = connection
        .query_row(
            "SELECT delta_link, next_link, last_sync_seconds, last_sync_nanoseconds,
                    replacement_active
             FROM calendar_view_sync_state
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
                scope.local_end_julian_day
            ],
            |row| {
                Ok(RawCalendarSyncState {
                    delta_link: row.get(0)?,
                    next_link: row.get(1)?,
                    last_sync_seconds: row.get(2)?,
                    last_sync_nanoseconds: row.get(3)?,
                    replacement_active: row.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or(CacheFailure::InvalidData)?;
    let last_synchronized_at = match (raw.last_sync_seconds, raw.last_sync_nanoseconds) {
        (None, None) => None,
        (Some(seconds), Some(nanoseconds)) => Some(system_time_from_parts(seconds, nanoseconds)?),
        _ => return Err(CacheFailure::InvalidData),
    };
    let mode = match raw.replacement_active {
        0 => CalendarSyncMode::Incremental,
        1 => CalendarSyncMode::Replacement,
        _ => return Err(CacheFailure::InvalidData),
    };
    CalendarSyncState::from_checkpoint(
        raw.delta_link.map(CalendarDeltaLink::from_opaque),
        raw.next_link.map(CalendarDeltaLink::from_opaque),
        last_synchronized_at,
        mode,
    )
    .map_err(|_| CacheFailure::InvalidData)
}

struct RawCalendarSyncState {
    delta_link: Option<String>,
    next_link: Option<String>,
    last_sync_seconds: Option<i64>,
    last_sync_nanoseconds: Option<i64>,
    replacement_active: i64,
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

fn read_pending_event_creation(
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

fn mark_pending_event_creation_rejected(
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

fn write_pending_event_creation(
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

fn confirms_calendar_event_creation(
    creation: &CalendarEventCreation,
    event: &CalendarOccurrence,
) -> bool {
    event.reference() == creation.view()
        && event.subject() == Some(creation.draft().subject())
        && event.time() == creation.draft().time()
        && event.overlaps_view()
}

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

fn delete_calendar_event_from(
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

fn commit_calendar_event_update(
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

fn commit_calendar_event_deletion(
    transaction: &Transaction<'_>,
    account_id: &str,
    source_range: CalendarScope,
    event: &EditableCalendarEvent,
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

fn write_calendar_event(
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

fn write_calendar_rebuild_event(
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

fn mark_calendar_rebuild_removed(
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

fn clear_calendar_rebuild_removed(
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

fn calendar_rebuild_event_was_removed(
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

fn read_calendar_event(
    connection: &Connection,
    scope: CalendarEventScope<'_>,
    event_id: &str,
) -> CacheResult<Option<CalendarOccurrence>> {
    read_calendar_event_from(connection, scope, event_id, CalendarEventTable::Active)
}

fn read_calendar_event_from(
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

fn read_calendar_events(
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

fn offset_datetime_parts(date_time: OffsetDateTime) -> (i64, i64) {
    (
        date_time.unix_timestamp(),
        i64::from(date_time.nanosecond()),
    )
}

fn offset_datetime_from_parts(seconds: i64, nanoseconds: i64) -> CacheResult<OffsetDateTime> {
    if !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(CacheFailure::InvalidData);
    }
    OffsetDateTime::from_unix_timestamp(seconds)
        .and_then(|date_time| date_time.replace_nanosecond(nanoseconds as u32))
        .map_err(|_| CacheFailure::InvalidData)
}
