use std::{collections::HashSet, future::Future, time::SystemTime};

use async_sqlite::rusqlite::{
    Connection, OptionalExtension, Transaction, params, params_from_iter,
};
use einfach_mail_application::{
    CalendarCache, CalendarCacheError, CalendarEventCreationStage, CalendarSyncMode,
    CalendarSyncState,
};
use einfach_mail_domain::{
    AccountId, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarEventCreation, CalendarEventCreationId, CalendarEventDraft, CalendarEventUpdate,
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, CalendarTimeZoneId, CalendarViewRange, CalendarViewReference,
    PendingCalendarEventCreation, VersionedCalendarEvent,
};
use thiserror::Error;
use time::{Date, OffsetDateTime, PrimitiveDateTime};

use crate::shared::store::{
    AccountScopeError, CacheFailure, CacheResult, SqliteAccountCache,
    time::{system_time_from_parts, system_time_parts},
};

mod events;
mod snapshot;
mod sync;
mod writes;

use self::{
    events::{
        offset_datetime_from_parts, offset_datetime_parts, read_calendar_event,
        read_calendar_event_from, read_calendar_events, write_calendar_event,
        write_calendar_rebuild_event,
    },
    snapshot::{
        begin_calendar_full_resync, clear_calendar_snapshot_state, commit_calendar_snapshot,
        replace_calendar_snapshot,
    },
    sync::{commit_calendar_page, insert_calendar_view_state, read_calendar_sync_state},
    writes::{
        calendar_account_has_active_replacement, calendar_account_has_write_sync_required,
        calendar_rebuild_event_was_removed, clear_calendar_rebuild_removed,
        clear_calendar_write_sync_required, commit_calendar_event_deletion,
        commit_calendar_event_update, confirms_calendar_event_creation, delete_calendar_event_from,
        mark_calendar_rebuild_removed, mark_calendar_write_sync_required,
        mark_pending_event_creation_rejected, read_pending_event_creation,
        write_pending_event_creation,
    },
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

struct CalendarSnapshotCommit {
    view: CalendarViewReference,
    scope: CalendarScope,
    events: Vec<CalendarOccurrence>,
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
    fn check_calendar_account(&self, account_id: &AccountId) -> Result<(), CalendarCacheError> {
        self.check_account_scope(account_id)
            .map_err(|AccountScopeError::AccountMismatch| CalendarCacheError::AccountMismatch)
    }

    fn check_calendar_view(
        &self,
        view: &CalendarViewReference,
    ) -> Result<CalendarScope, CalendarCacheError> {
        if &self.account_id != view.account_id() {
            return Err(CalendarCacheError::AccountMismatch);
        }
        Ok(CalendarScope::from_view(view))
    }

    fn validate_calendar_snapshot(
        view: &CalendarViewReference,
        events: &[CalendarOccurrence],
    ) -> Result<(), CalendarCacheError> {
        let mut event_ids = HashSet::with_capacity(events.len());
        for event in events {
            if event.reference().account_id() != view.account_id() {
                return Err(CalendarCacheError::AccountMismatch);
            }
            if event.reference() != view
                || !event.overlaps_view()
                || !event_ids.insert(event.id().as_str())
            {
                return Err(CalendarCacheError::InvalidData);
            }
        }
        Ok(())
    }

    fn prepare_calendar_snapshot_commit(
        &self,
        view: &CalendarViewReference,
        events: Vec<CalendarOccurrence>,
        completed_at: SystemTime,
    ) -> Result<CalendarSnapshotCommit, CalendarCacheError> {
        let scope = self.check_calendar_view(view)?;
        Self::validate_calendar_snapshot(view, &events)?;
        let completed_at =
            system_time_parts(completed_at).map_err(CacheFailure::calendar_public)?;
        Ok(CalendarSnapshotCommit {
            view: view.clone(),
            scope,
            events,
            completed_at,
        })
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
        let validation = self.check_calendar_account(account_id);
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
        let validation = self.check_calendar_account(account_id);
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
        let validation = self.check_calendar_account(account_id);
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

    fn replace_snapshot(
        &self,
        view: &CalendarViewReference,
        events: Vec<CalendarOccurrence>,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let commit = self.prepare_calendar_snapshot_commit(view, events, completed_at);
        let client = self.client.clone();
        async move {
            let commit = commit?;
            client
                .conn_mut_and_then(move |connection| commit_calendar_snapshot(connection, commit))
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
    ) -> impl Future<Output = Result<CalendarEventCreationStage, CalendarCacheError>> + Send + '_
    {
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
                    let stage = match read_pending_event_creation(&transaction, &account_id)? {
                        Some(PendingCalendarEventCreation::Unresolved(existing))
                            if existing == creation =>
                        {
                            CalendarEventCreationStage::ExistingUnresolved
                        }
                        Some(_) => return Err(CalendarCacheFailure::CreationAlreadyPending),
                        None => {
                            write_pending_event_creation(&transaction, &creation)?;
                            CalendarEventCreationStage::NewlyStaged
                        }
                    };
                    transaction.commit()?;
                    Ok(stage)
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
                    if !matches!(
                        pending.as_ref(),
                        Some(PendingCalendarEventCreation::Unresolved(existing))
                            if existing == &creation
                    ) {
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
        event: &VersionedCalendarEvent,
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

#[derive(Clone, Copy, Eq, PartialEq)]
enum CalendarEventTable {
    Active,
    Rebuild,
}
