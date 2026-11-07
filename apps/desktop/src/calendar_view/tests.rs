use std::{
    collections::HashMap,
    future::{Future, ready},
    ops::Deref,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use einfach_mail_application::{
    CalendarCache, CalendarCacheError, CalendarDeltaPageResponse, CalendarDeltaRequest,
    CalendarDeltaResponse, CalendarOperationError, CalendarProvider, CalendarSession,
    CalendarSyncMode, CalendarSyncState, CalendarWorkflow, CalendarWriteReadiness,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarEventCreation, CalendarEventUpdate, CalendarEventVersion, CalendarOccurrence,
    CalendarOccurrencePatch, CalendarOccurrenceTime, CalendarOccurrenceTimePatch,
    CalendarViewRange, CalendarViewReference, EditableCalendarEvent, EventId,
    PendingCalendarEventCreation,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{AppContext as _, Modifiers, TestAppContext, VisualTestContext, px, size};
use jiff::{Span, civil, tz::TimeZone};
use time::{Date, Month, OffsetDateTime};
use tokio::sync::oneshot;

use super::{CalendarView, current_week_start, view_for_week};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

#[derive(Clone, Default)]
struct TestProvider {
    requests: Arc<Mutex<Vec<(i64, i64)>>>,
    calendar_sync_succeeds: Arc<AtomicBool>,
    calendar_sync_failure: Arc<Mutex<Option<CalendarOperationError>>>,
    event_creation_gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    event_creation_failure: Arc<Mutex<Option<CalendarOperationError>>>,
    event_creation_count: Arc<AtomicUsize>,
    event_update_calls: Arc<AtomicUsize>,
    event_update_gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    event_update_failure: Arc<Mutex<Option<CalendarOperationError>>>,
    event_deletion_calls: Arc<AtomicUsize>,
    event_deletion_gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    event_deletion_failure: Arc<Mutex<Option<CalendarOperationError>>>,
}

impl TestProvider {
    fn allow_calendar_sync(&self) {
        self.calendar_sync_succeeds.store(true, Ordering::Release);
    }

    fn fail_next_calendar_sync(&self, failure: CalendarOperationError) {
        *self
            .calendar_sync_failure
            .lock()
            .expect("synthetic calendar sync failure mutex is not poisoned") = Some(failure);
    }

    fn hold_event_update_until(&self, gate: oneshot::Receiver<()>) {
        *self
            .event_update_gate
            .lock()
            .expect("synthetic update gate mutex is not poisoned") = Some(gate);
    }

    fn fail_next_event_update(&self, failure: CalendarOperationError) {
        *self
            .event_update_failure
            .lock()
            .expect("synthetic update failure mutex is not poisoned") = Some(failure);
    }

    fn hold_event_deletion_until(&self, gate: oneshot::Receiver<()>) {
        *self
            .event_deletion_gate
            .lock()
            .expect("synthetic deletion gate mutex is not poisoned") = Some(gate);
    }

    fn fail_next_event_deletion(&self, failure: CalendarOperationError) {
        *self
            .event_deletion_failure
            .lock()
            .expect("synthetic deletion failure mutex is not poisoned") = Some(failure);
    }

    fn hold_event_creation_until(&self, gate: oneshot::Receiver<()>) {
        *self
            .event_creation_gate
            .lock()
            .expect("synthetic creation gate mutex is not poisoned") = Some(gate);
    }

    fn fail_next_event_creation(&self, failure: CalendarOperationError) {
        *self
            .event_creation_failure
            .lock()
            .expect("synthetic creation failure mutex is not poisoned") = Some(failure);
    }

    fn take_event_creation_gate(&self) -> Option<oneshot::Receiver<()>> {
        self.event_creation_gate
            .lock()
            .expect("synthetic creation gate mutex is not poisoned")
            .take()
    }

    fn take_event_creation_failure(&self) -> Option<CalendarOperationError> {
        self.event_creation_failure
            .lock()
            .expect("synthetic creation failure mutex is not poisoned")
            .take()
    }
}

impl CalendarProvider for TestProvider {
    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        _: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        self.requests
            .lock()
            .expect("synthetic calendar request log mutex is not poisoned")
            .push((
                view.range().start().unix_timestamp(),
                view.range().end().unix_timestamp(),
            ));
        let failure = self
            .calendar_sync_failure
            .lock()
            .expect("synthetic calendar sync failure mutex is not poisoned")
            .take();
        let result = if let Some(failure) = failure {
            Err(failure)
        } else if self.calendar_sync_succeeds.load(Ordering::Acquire) {
            Ok(CalendarDeltaResponse::Page(CalendarDeltaPageResponse::new(
                CalendarDeltaPage::new(
                    Vec::new(),
                    CalendarDeltaProgress::Complete(CalendarDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken=synthetic-ui"
                            .to_owned(),
                    )),
                ),
                0,
            )))
        } else {
            Err(CalendarOperationError::AccessDenied)
        };
        ready(result)
    }

    fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let gate = self.take_event_creation_gate();
        let failure = self.take_event_creation_failure();
        let creation = creation.clone();
        let event_creation_count = Arc::clone(&self.event_creation_count);
        async move {
            event_creation_count.fetch_add(1, Ordering::SeqCst);
            create_event_after_wait(creation, gate, failure).await
        }
    }

    fn editable_event(
        &self,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<EditableCalendarEvent, CalendarOperationError>> + Send + '_
    {
        let event = event.clone();
        async move {
            EditableCalendarEvent::new(
                event,
                CalendarEventVersion::try_from("synthetic-version-1".to_owned())
                    .map_err(|_| CalendarOperationError::InvalidResponse)?,
            )
            .map_err(|_| CalendarOperationError::EventNotEditable)
        }
    }

    fn update_event(
        &self,
        update: &CalendarEventUpdate,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let update = update.clone();
        let gate = self
            .event_update_gate
            .lock()
            .expect("synthetic update gate mutex is not poisoned")
            .take();
        let failure = self
            .event_update_failure
            .lock()
            .expect("synthetic update failure mutex is not poisoned")
            .take();
        let event_update_calls = Arc::clone(&self.event_update_calls);
        async move {
            event_update_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = gate {
                gate.await
                    .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
            }
            if let Some(failure) = failure {
                return Err(failure);
            }
            let mut event = update.source().occurrence().clone();
            let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
                return Err(CalendarOperationError::InvalidResponse);
            };
            let mut patch = CalendarOccurrencePatch::new();
            patch.set_subject(Some(update.draft().subject().to_owned()));
            let mut time = CalendarOccurrenceTimePatch::new();
            time.set_start(*start);
            time.set_end(*end);
            time.set_is_all_day(false);
            patch.set_time(time);
            patch
                .apply_to(&mut event)
                .map_err(|_| CalendarOperationError::InvalidResponse)?;
            Ok(event)
        }
    }

    fn delete_event(
        &self,
        _: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        let gate = self
            .event_deletion_gate
            .lock()
            .expect("synthetic deletion gate mutex is not poisoned")
            .take();
        let failure = self
            .event_deletion_failure
            .lock()
            .expect("synthetic deletion failure mutex is not poisoned")
            .take();
        let event_deletion_calls = Arc::clone(&self.event_deletion_calls);
        async move {
            event_deletion_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = gate {
                gate.await
                    .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
            }
            failure.map_or(Ok(()), Err)
        }
    }
}

async fn create_event_after_wait(
    creation: CalendarEventCreation,
    gate: Option<oneshot::Receiver<()>>,
    failure: Option<CalendarOperationError>,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    if let Some(gate) = gate {
        gate.await
            .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
    }
    match failure {
        Some(failure) => Err(failure),
        None => created_event(&creation),
    }
}

fn created_event(
    creation: &CalendarEventCreation,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        return Err(CalendarOperationError::InvalidResponse);
    };
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(creation.draft().subject().to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(*start);
    time.set_end(*end);
    time.set_is_all_day(false);
    patch.set_time(time);
    let id = EventId::try_from(format!(
        "synthetic-created-event-{}",
        creation.id().as_uuid()
    ))
    .map_err(|_| CalendarOperationError::InvalidResponse)?;
    patch
        .into_new(creation.view().clone(), id)
        .map_err(|_| CalendarOperationError::InvalidResponse)
}

#[derive(Clone, Default)]
struct TestCache {
    events: Option<Vec<CalendarOccurrence>>,
    created_events: Arc<Mutex<HashMap<CalendarViewReference, HashMap<String, CalendarOccurrence>>>>,
    pending_creation: Arc<Mutex<Option<PendingCalendarEventCreation>>>,
    fail_next_rejected_cleanup: Arc<AtomicBool>,
    updated_events: Arc<Mutex<HashMap<CalendarViewReference, HashMap<String, CalendarOccurrence>>>>,
    deleted_event_ids: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl TestCache {
    fn with_events(events: Vec<CalendarOccurrence>) -> Self {
        Self {
            events: Some(events),
            ..Self::default()
        }
    }

    fn fail_rejected_cleanup_once(&self) {
        self.fail_next_rejected_cleanup
            .store(true, Ordering::SeqCst);
    }
}

impl CalendarCache for TestCache {
    fn ensure_view(
        &self,
        _: &CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        ready(Ok(CalendarSyncState::initial()))
    }

    fn account_has_active_replacement(
        &self,
        _: &AccountId,
    ) -> impl Future<Output = Result<bool, CalendarCacheError>> + Send + '_ {
        ready(Ok(false))
    }

    fn commit_delta_page(
        &self,
        _: &CalendarViewReference,
        page: CalendarDeltaPage,
        completed_at: std::time::SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let (_, progress) = page.into_parts();
        let state = match progress {
            CalendarDeltaProgress::Next(link) => CalendarSyncState::from_checkpoint(
                None,
                Some(link),
                None,
                CalendarSyncMode::Incremental,
            ),
            CalendarDeltaProgress::Complete(link) => CalendarSyncState::from_checkpoint(
                Some(link),
                None,
                Some(completed_at),
                CalendarSyncMode::Incremental,
            ),
        };
        ready(state)
    }

    fn begin_full_resync(
        &self,
        _: &CalendarViewReference,
        _: Option<CalendarDeltaLink>,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        ready(Err(CalendarCacheError::InvalidData))
    }

    fn list_events(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<Vec<CalendarOccurrence>, CalendarCacheError>> + Send + '_ {
        if let Some(events) = &self.events {
            return ready(Ok(events.clone()));
        }
        let created_events = self
            .created_events
            .lock()
            .expect("synthetic event cache mutex is not poisoned")
            .get(view)
            .map(|events| events.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let updated_events = self
            .updated_events
            .lock()
            .expect("synthetic updated event mutex is not poisoned")
            .get(view)
            .map(|events| events.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some("Synthetic offline event".to_owned()));
        patch.set_location(Some("Synthetic room".to_owned()));
        let mut time = CalendarOccurrenceTimePatch::new();
        time.set_start(view.range().start_date().midnight().assume_utc());
        let end = view
            .range()
            .start_date()
            .checked_add(time::Duration::days(1))
            .expect("synthetic event end date is valid");
        time.set_end(end.midnight().assume_utc());
        time.set_is_all_day(true);
        patch.set_time(time);
        let event = patch
            .into_new(
                view.clone(),
                EventId::try_from("synthetic-ui-event".to_owned())
                    .expect("synthetic event ID is valid"),
            )
            .expect("synthetic cached event has a complete time range");
        let mut events = vec![
            event,
            timed_event(
                Date::from_calendar_date(2030, Month::January, 9)
                    .expect("synthetic event start date is valid")
                    .with_time(time::Time::from_hms(10, 0, 0).expect("valid time"))
                    .assume_utc(),
                Date::from_calendar_date(2030, Month::January, 9)
                    .expect("synthetic event end date is valid")
                    .with_time(time::Time::from_hms(11, 0, 0).expect("valid time"))
                    .assume_utc(),
            ),
        ];
        events.retain(|cached| {
            !updated_events
                .iter()
                .any(|updated| updated.id() == cached.id())
        });
        events.extend(updated_events);
        events.extend(created_events);
        let deleted_event_ids = self
            .deleted_event_ids
            .lock()
            .expect("synthetic deleted event set is not poisoned")
            .clone();
        events.retain(|event| !deleted_event_ids.contains(event.id().as_str()));
        ready(Ok(events))
    }

    fn stage_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let pending_creation = Arc::clone(&self.pending_creation);
        let creation = creation.clone();
        async move {
            let mut pending = pending_creation
                .lock()
                .expect("synthetic pending event mutex is not poisoned");
            match pending.as_ref() {
                Some(PendingCalendarEventCreation::Unresolved(existing))
                    if existing == &creation =>
                {
                    Ok(())
                }
                None => {
                    *pending = Some(PendingCalendarEventCreation::Unresolved(creation));
                    Ok(())
                }
                Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
            }
        }
    }

    fn pending_event_creation(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Option<PendingCalendarEventCreation>, CalendarCacheError>> + Send + '_
    {
        let pending_creation = Arc::clone(&self.pending_creation);
        let account_id = account_id.clone();
        async move {
            let pending = pending_creation
                .lock()
                .expect("synthetic pending event mutex is not poisoned")
                .clone();
            if pending
                .as_ref()
                .is_some_and(|pending| pending.creation().view().account_id() != &account_id)
            {
                Err(CalendarCacheError::AccountMismatch)
            } else {
                Ok(pending)
            }
        }
    }

    fn mark_event_creation_rejected(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let pending_creation = Arc::clone(&self.pending_creation);
        let creation = creation.clone();
        async move {
            let mut pending = pending_creation
                .lock()
                .expect("synthetic pending event mutex is not poisoned");
            match pending.as_ref() {
                Some(PendingCalendarEventCreation::Unresolved(existing))
                    if existing == &creation =>
                {
                    *pending = Some(PendingCalendarEventCreation::Rejected(creation));
                    Ok(())
                }
                Some(PendingCalendarEventCreation::Rejected(existing)) if existing == &creation => {
                    Ok(())
                }
                None => Ok(()),
                Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
            }
        }
    }

    fn discard_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let pending_creation = Arc::clone(&self.pending_creation);
        let fail_cleanup = Arc::clone(&self.fail_next_rejected_cleanup);
        let creation = creation.clone();
        async move {
            if fail_cleanup.swap(false, Ordering::SeqCst) {
                return Err(CalendarCacheError::Unavailable);
            }
            let mut pending = pending_creation
                .lock()
                .expect("synthetic pending event mutex is not poisoned");
            match pending.as_ref() {
                Some(PendingCalendarEventCreation::Rejected(existing)) if existing == &creation => {
                    *pending = None;
                    Ok(())
                }
                None => Ok(()),
                Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
            }
        }
    }

    fn commit_event_creation(
        &self,
        creation: &CalendarEventCreation,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let created_events = Arc::clone(&self.created_events);
        let creation = creation.clone();
        let event = event.clone();
        let pending_creation = Arc::clone(&self.pending_creation);
        async move {
            if event.reference() != creation.view()
                || event.subject() != Some(creation.draft().subject())
                || event.time() != creation.draft().time()
            {
                return Err(CalendarCacheError::InvalidData);
            }
            let mut pending = pending_creation
                .lock()
                .expect("synthetic pending event mutex is not poisoned");
            if pending.as_ref() != Some(&PendingCalendarEventCreation::Unresolved(creation.clone()))
            {
                return Err(CalendarCacheError::InvalidData);
            }
            created_events
                .lock()
                .expect("synthetic event cache mutex is not poisoned")
                .entry(creation.view().clone())
                .or_default()
                .insert(event.id().as_str().to_owned(), event);
            *pending = None;
            Ok(())
        }
    }

    fn commit_event_update(
        &self,
        update: &CalendarEventUpdate,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let updated_events = Arc::clone(&self.updated_events);
        let update = update.clone();
        let event = event.clone();
        async move {
            if !update.confirms(&event) {
                return Err(CalendarCacheError::InvalidData);
            }
            updated_events
                .lock()
                .expect("synthetic updated event mutex is not poisoned")
                .entry(event.reference().clone())
                .or_default()
                .insert(event.id().as_str().to_owned(), event);
            Ok(())
        }
    }

    fn commit_event_deletion(
        &self,
        event: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let deleted_event_ids = Arc::clone(&self.deleted_event_ids);
        let event_id = event.occurrence().id().as_str().to_owned();
        async move {
            deleted_event_ids
                .lock()
                .expect("synthetic deleted event set is not poisoned")
                .insert(event_id);
            Ok(())
        }
    }
}

fn profile() -> AccountProfile {
    AccountProfile::try_new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        None,
    )
    .expect("synthetic account profile is valid")
}

fn timed_event(start: OffsetDateTime, end: OffsetDateTime) -> CalendarOccurrence {
    timed_event_with_id("synthetic-timed-event", start, end)
}

fn timed_event_with_id(
    event_id: &str,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> CalendarOccurrence {
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let start_date = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic view start date is valid");
    let end_date = Date::from_calendar_date(2030, Month::January, 14)
        .expect("synthetic view end date is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic calendar range is valid");
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Synthetic overnight event".to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(start);
    time.set_end(end);
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
        .into_new(
            CalendarViewReference::new(account, range),
            EventId::try_from(event_id.to_owned()).expect("synthetic event ID is valid"),
        )
        .expect("synthetic timed event is valid")
}

fn agenda_events(count: usize) -> Vec<CalendarOccurrence> {
    let date = Date::from_calendar_date(2030, Month::January, 7).expect("synthetic date is valid");
    (0..count)
        .map(|index| {
            let event_id = format!("synthetic-calendar-event-{}", index + 1);
            let start_minutes = 8 * 60 + i32::try_from(index).expect("event index fits") * 5;
            let start = date.midnight().assume_utc().replace_time(
                time::Time::from_hms(
                    u8::try_from(start_minutes / 60).expect("hour fits"),
                    u8::try_from(start_minutes % 60).expect("minute fits"),
                    0,
                )
                .expect("synthetic event time is valid"),
            );
            timed_event_with_id(&event_id, start, start + time::Duration::minutes(30))
        })
        .collect()
}

fn draw_calendar_view(context: &mut VisualTestContext) {
    context.update(|window, app| {
        let _ = window.draw(app);
    });
}

fn select_default_timed_event(context: &mut VisualTestContext) {
    let event = context
        .debug_bounds("calendar-event-row-2030-01-09-synthetic-timed-event")
        .expect("the default calendar cache contains a timed event");
    context.simulate_click(event.center(), Modifiers::default());
}

#[test]
fn calendar_view_bounds_follow_local_monday_across_a_dst_transition() {
    let time_zone = TimeZone::get("America/New_York")
        .expect("the synthetic IANA zone exists in Linux zoneinfo");
    let sunday = civil::Date::new(2030, 3, 10).expect("synthetic Sunday is valid");
    let start = super::monday_start(sunday).expect("Monday is in range");
    assert_eq!(start, civil::Date::new(2030, 3, 4).expect("valid date"));
    let account = einfach_mail_domain::AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is valid");
    let (view, _) =
        view_for_week(&account, start, &time_zone).expect("synthetic local week has UTC bounds");
    let range = view.range();
    assert_eq!(range.start_date().day(), 4);
    assert_eq!(range.end_date_exclusive().day(), 11);
    assert_eq!(
        range.end() - range.start(),
        time::Duration::days(7) - time::Duration::hours(1)
    );
}

#[test]
fn utc_calendar_week_uses_monday_start_and_utc_offset() {
    let current = current_week_start(&TimeZone::UTC).expect("current timestamp fits Jiff");
    let account = einfach_mail_domain::AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is valid");
    let (view, week_start) =
        view_for_week(&account, current, &TimeZone::UTC).expect("synthetic UTC week is valid");

    assert_eq!(week_start.weekday(), jiff::civil::Weekday::Monday);
    assert_eq!(view.range().start().offset(), time::UtcOffset::UTC);
}

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

#[gpui_kit::test]
async fn calendar_agenda_navigates_events_with_one_focus_stop_and_keeps_selection_visible(
    cx: &mut TestAppContext,
) {
    const AGENDA_EVENT_COUNT: usize = 20;
    const FIRST_OPTION_LABEL: &str =
        "Monday, January 7, 08:00 UTC–08:30 UTC, Synthetic overnight event";
    const SECOND_OPTION_LABEL: &str =
        "Monday, January 7, 08:05 UTC–08:35 UTC, Synthetic overnight event";
    const LAST_OPTION_LABEL: &str =
        "Monday, January 7, 09:35 UTC–10:05 UTC, Synthetic overnight event";

    cx.update(|cx| {
        gpui_kit::init(cx);
        super::bind_calendar_event_navigation_keys(cx);
    });
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let workflow = Arc::new(CalendarWorkflow::new(
        TestProvider::default(),
        TestCache::with_events(agenda_events(AGENDA_EVENT_COUNT)),
    ));
    let mut calendar_view = None;
    let window = cx.open_window(size(px(1280.), px(800.)), |window, cx| {
        let view = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                super::CalendarWeekSelection {
                    week_start,
                    time_zone: TimeZone::UTC,
                    utc_fallback: false,
                },
                cx,
            )
        });
        calendar_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    cx.run_until_parked();
    let context = VisualTestContext::from_window(*window.deref(), cx).into_mut();
    let view = calendar_view.expect("calendar view is created with the test window");

    wait_for_ui_condition(context, |_, app| !(view.read(app).sync_running)).await;
    assert_accessible_agenda(context, AGENDA_EVENT_COUNT);
    assert!(context.debug_bounds("calendar-event-list").is_some());
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).selected_agenda_event.is_none())
    );
    assert!(context.debug_bounds("calendar-event-edit").is_none());

    context.update(gpui_kit::Window::focus_next);
    let mut list_focused =
        context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window));
    for _ in 0..24 {
        if list_focused {
            break;
        }
        context.simulate_keystrokes("tab");
        list_focused =
            context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window));
        if list_focused {
            break;
        }
    }
    assert!(list_focused, "Tab reaches the calendar event list");
    assert_eq!(accessible_agenda_list_focus(context), Some(true));

    context.simulate_keystrokes("up");
    assert_selected_event(context, &view, "synthetic-calendar-event-20");
    assert_accessible_selection(context, LAST_OPTION_LABEL);
    let edit_bounds = context
        .debug_bounds("calendar-event-edit")
        .expect("selecting a timed event exposes its edit command");
    let list_bounds = context
        .debug_bounds("calendar-event-list")
        .expect("event list is rendered");
    assert!(
        edit_bounds.bottom() <= list_bounds.top(),
        "the edit command stays outside listbox options; edit={edit_bounds:?}, list={list_bounds:?}"
    );
    context.simulate_keystrokes("down");
    assert_selected_event(context, &view, "synthetic-calendar-event-20");
    assert_accessible_selection(context, LAST_OPTION_LABEL);

    context.simulate_keystrokes("home");
    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert_accessible_selection(context, FIRST_OPTION_LABEL);
    context.simulate_keystrokes("up");
    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert_accessible_selection(context, FIRST_OPTION_LABEL);
    context.simulate_keystrokes("down");
    assert_selected_event(context, &view, "synthetic-calendar-event-2");
    assert_accessible_selection(context, SECOND_OPTION_LABEL);
    context.simulate_keystrokes("end");
    context.run_until_parked();
    draw_calendar_view(context);
    assert_selected_event(context, &view, "synthetic-calendar-event-20");
    assert_accessible_selection(context, LAST_OPTION_LABEL);

    let list_bounds = context
        .debug_bounds("calendar-event-list")
        .expect("event list is rendered");
    let selected_bounds = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-20")
        .expect("End scrolls the last event into view");
    assert!(
        selected_bounds.top() >= list_bounds.top(),
        "selected event starts above the agenda viewport; list={list_bounds:?}, selected={selected_bounds:?}"
    );
    assert!(
        selected_bounds.bottom() <= list_bounds.bottom(),
        "selected event ends below the agenda viewport; list={list_bounds:?}, selected={selected_bounds:?}"
    );
    let (scroll_offset, max_scroll_offset) = context.cx.update(|cx| {
        let scroll = &view.read(cx).agenda_scroll;
        (scroll.offset(), scroll.max_offset())
    });
    assert!(
        max_scroll_offset.y > px(0.),
        "the short test viewport should require agenda scrolling; max={max_scroll_offset:?}, list={list_bounds:?}"
    );
    assert!(
        scroll_offset.y < px(0.),
        "End should scroll the agenda down; offset={scroll_offset:?}, max={max_scroll_offset:?}"
    );

    context.cx.update(|cx| {
        view.update(cx, |view, _| view.agenda_scroll.scroll_to_item(0));
    });
    context.run_until_parked();
    draw_calendar_view(context);
    context.simulate_keystrokes("tab");
    assert!(
        !context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window)),
        "Tab leaves the event list as one focus stop"
    );
    assert_eq!(accessible_agenda_list_focus(context), Some(false));
    let first_row = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-1")
        .expect("the first event is visible after Home");
    context.simulate_click(first_row.center(), Modifiers::default());
    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert!(context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window)));
    assert_eq!(accessible_agenda_list_focus(context), Some(true));
    assert_accessible_selection(context, FIRST_OPTION_LABEL);
}

#[gpui_kit::test]
async fn selecting_an_event_uses_its_current_row_after_agenda_rows_shift(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let workflow = Arc::new(CalendarWorkflow::new(
        TestProvider::default(),
        TestCache::with_events(agenda_events(20)),
    ));
    let mut calendar_view = None;
    let window = cx.open_window(size(px(1280.), px(480.)), |window, cx| {
        let view = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                super::CalendarWeekSelection {
                    week_start,
                    time_zone: TimeZone::UTC,
                    utc_fallback: false,
                },
                cx,
            )
        });
        calendar_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    cx.run_until_parked();
    let context = VisualTestContext::from_window(*window.deref(), cx).into_mut();
    let view = calendar_view.expect("calendar view is created with the test window");

    wait_for_ui_condition(context, |_, app| !(view.read(app).sync_running)).await;
    draw_calendar_view(context);
    assert!(
        context
            .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-1")
            .is_some(),
        "the first event is rendered before the rows change"
    );
    let target_event = super::AgendaEventKey {
        date: civil::Date::new(2030, 1, 7).expect("synthetic event date is valid"),
        event_id: EventId::try_from("synthetic-calendar-event-1".to_owned())
            .expect("synthetic event ID is valid"),
    };
    let old_scroll_index = context.cx.update(|cx| {
        view.read(cx).agenda_rows().iter().position(
            |row| matches!(row, super::AgendaRow::Event(event) if event.key == target_event),
        )
    });
    let old_scroll_index = old_scroll_index.expect("the target event has an agenda row");
    let date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic event date is valid");
    let earlier_start = date
        .midnight()
        .assume_utc()
        .replace_time(time::Time::from_hms(6, 0, 0).expect("synthetic time is valid"));
    let earlier_events = (0..24)
        .map(|index| {
            let minutes = i64::from(index);
            let start = earlier_start + time::Duration::minutes(minutes);
            let event_id = format!("synthetic-inserted-event-{index}");
            timed_event_with_id(&event_id, start, start + time::Duration::minutes(15))
        })
        .collect::<Vec<_>>();

    context.cx.update(|cx| {
        view.update(cx, |view, _| {
            view.events.extend(earlier_events);
        });
    });
    let current_scroll_index = context.cx.update(|cx| {
        view.read(cx).agenda_rows().iter().position(
            |row| matches!(row, super::AgendaRow::Event(event) if event.key == target_event),
        )
    });
    let current_scroll_index =
        current_scroll_index.expect("the target event remains in the changed agenda");
    assert_eq!(current_scroll_index, old_scroll_index + 24);
    context.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.select_event_from_option(target_event, window, cx);
        });
    });
    context.run_until_parked();
    draw_calendar_view(context);

    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert!(context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window)));
    let list_bounds = context
        .debug_bounds("calendar-event-list")
        .expect("event list remains rendered");
    let selected_bounds = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-1")
        .expect("the selected event remains rendered");
    assert!(
        selected_bounds.top() >= list_bounds.top()
            && selected_bounds.bottom() <= list_bounds.bottom(),
        "selecting after earlier rows are inserted scrolls the event into view; list={list_bounds:?}, selected={selected_bounds:?}"
    );
    let scroll_offset = context.cx.update(|cx| view.read(cx).agenda_scroll.offset());
    assert!(
        scroll_offset.y < px(0.),
        "the shifted event row requires scrolling; offset={scroll_offset:?}"
    );
}

#[gpui_kit::test]
async fn native_calendar_view_keeps_cached_events_and_navigates_fixed_weeks(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let time_zone = TimeZone::UTC;
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let provider = TestProvider::default();
    let requests = Arc::clone(&provider.requests);
    let workflow = Arc::new(CalendarWorkflow::new(provider, TestCache::default()));
    let (view, context) = cx.add_window_view(|_, cx| {
        CalendarView::new_for_week(
            profile(),
            workflow,
            super::CalendarWeekSelection {
                week_start,
                time_zone,
                utc_fallback: true,
            },
            cx,
        )
    });

    context.run_until_parked();
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "calendar-sync-error")
    })
    .await;
    let (sync_running, status_failed) = context.cx.update(|cx| {
        let view = view.read(cx);
        (
            view.sync_running,
            matches!(view.status, super::CalendarStatus::Failed(_)),
        )
    });
    assert!(!sync_running && status_failed, "calendar refresh completes");
    assert!(context.debug_bounds("calendar-sync-error").is_some());
    assert!(context.debug_bounds("calendar-utc-fallback").is_some());
    let status_labels = context.update(|window, _| {
        window
            .find_all_by_role(gpui_kit::Role::Status)
            .iter()
            .map(|status| status.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(
        status_labels.iter().any(|label| {
            label.as_deref()
                == Some("The system time zone is unavailable. Timed events are shown in UTC.")
        }),
        "the UTC fallback status label is accessible; found {status_labels:?}"
    );
    assert!(
        context
            .debug_bounds("calendar-event-row-2030-01-07-synthetic-ui-event")
            .is_some()
    );
    assert!(context.debug_bounds("calendar-next-week").is_some());

    let next_week_button = context
        .debug_bounds("calendar-next-week")
        .expect("week navigation button is rendered");
    context.simulate_click(next_week_button.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);

    let (selected_week, range) = context.cx.update(|cx| {
        let view = view.read(cx);
        (view.week_start, view.view.range().clone())
    });
    assert_eq!(
        selected_week,
        week_start
            .checked_add(Span::new().weeks(1))
            .expect("following week is in range")
    );
    assert_eq!(
        range.start_date(),
        Date::from_calendar_date(2030, Month::January, 14).expect("valid date")
    );
    assert_eq!(
        requests
            .lock()
            .expect("synthetic calendar request log mutex is not poisoned")
            .len(),
        2
    );
}

#[gpui_kit::test]
async fn native_event_composer_recovers_uncertain_and_rejected_creations(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let cache = TestCache::default();
    let workflow = Arc::new(CalendarWorkflow::new(provider.clone(), cache.clone()));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let mut calendar_handle = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let calendar = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                Arc::clone(&workflow),
                super::CalendarWeekSelection {
                    week_start,
                    time_zone: TimeZone::UTC,
                    utc_fallback: true,
                },
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "calendar-sync-error")
    })
    .await;
    assert!(
        context.debug_bounds("calendar-sync-error").is_some(),
        "calendar sync reaches a visible error state"
    );

    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("calendar offers event creation");
    context.simulate_click(new_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(!context.update(|window, cx| window.has_active_sheet(cx)));
    provider.allow_calendar_sync();
    let sync = context
        .debug_bounds("calendar-sync")
        .expect("calendar offers a retry after the initial sync fails");
    context.simulate_click(sync.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(context.cx.update(|cx| {
        calendar.read(cx).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate)
    }));
    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("synchronized calendar offers event creation");
    context.simulate_click(new_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.update(|window, cx| window.has_active_sheet(cx)));
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    assert!(context.debug_bounds("event-composer-fields").is_some());
    let create_event = context
        .debug_bounds("event-composer-submit")
        .expect("composer offers create action");
    context.simulate_click(create_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-composer-validation").is_some());
    assert!(context.debug_bounds("event-creation-pending").is_none());

    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("composer exposes the title input");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_input("Synthetic event");
    let (release, gate) = oneshot::channel();
    provider.hold_event_creation_until(gate);
    provider.fail_next_event_creation(CalendarOperationError::ServiceUnavailable);
    let create_event = context
        .debug_bounds("event-composer-submit")
        .expect("composer can submit the completed draft");
    context.simulate_click(create_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-creation-submitting").is_some());
    release
        .send(())
        .expect("synthetic provider request is waiting on this test");

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
    })
    .await;
    assert!(context.debug_bounds("event-creation-pending").is_some());
    assert!(context.debug_bounds("event-creation-status").is_some());

    let retry = context
        .debug_bounds("event-composer-submit")
        .expect("the saved operation remains retryable");
    context.simulate_click(retry.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
    })
    .await;
    assert!(context.debug_bounds("event-creation-pending").is_none());
    assert!(context.debug_bounds("event-creation-status").is_some());
    assert!(context.debug_bounds("event-composer-close").is_some());

    let close = context
        .debug_bounds("event-composer-close")
        .expect("confirmed composer can be closed");
    context.simulate_click(close.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);

    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("calendar offers another event creation");
    context.simulate_click(new_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-composer-fields").is_some());
    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("composer exposes the title input");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_input("Rejected synthetic event");
    provider.fail_next_event_creation(CalendarOperationError::EventCreationRejected);
    cache.fail_rejected_cleanup_once();

    let create_event = context
        .debug_bounds("event-composer-submit")
        .expect("composer offers create action");
    context.simulate_click(create_event.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
    })
    .await;
    assert!(context.debug_bounds("event-creation-pending").is_some());
    assert!(context.debug_bounds("event-creation-status").is_some());
    let rejected_marker = workflow
        .load_cached_view(
            &view_for_week(profile().id(), week_start, &TimeZone::UTC)
                .expect("synthetic calendar week is valid")
                .0,
        )
        .await
        .expect("rejected operation remains available for cleanup");
    assert!(matches!(
        rejected_marker.pending_creation(),
        Some(PendingCalendarEventCreation::Rejected(_))
    ));
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 3);

    let clear_rejected = context
        .debug_bounds("event-composer-submit")
        .expect("rejected operation offers local cleanup");
    context.simulate_click(clear_rejected.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
            && !test_element_is_visible(window, "event-creation-pending")
    })
    .await;
    assert!(context.debug_bounds("event-creation-pending").is_none());
    assert!(context.debug_bounds("event-composer-close").is_some());
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 3);
    let cleared_marker = workflow
        .load_cached_view(
            &view_for_week(profile().id(), week_start, &TimeZone::UTC)
                .expect("synthetic calendar week is valid")
                .0,
        )
        .await
        .expect("calendar view can read its cache");
    assert!(cleared_marker.pending_creation().is_none());
}

#[gpui_kit::test]
async fn denied_calendar_write_emits_permission_review_request(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    provider.fail_next_event_creation(CalendarOperationError::AccessDenied);
    let workflow = Arc::new(CalendarWorkflow::new(
        provider.clone(),
        TestCache::default(),
    ));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let permission_review_requested = Arc::new(AtomicBool::new(false));
    let requested = Arc::clone(&permission_review_requested);
    let mut calendar_handle = None;
    let mut subscription = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let calendar = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                super::CalendarWeekSelection {
                    week_start,
                    time_zone: TimeZone::UTC,
                    utc_fallback: true,
                },
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        subscription = Some(cx.subscribe(
            &calendar,
            move |_, _, event: &super::CalendarViewEvent, _| {
                if matches!(
                    event,
                    super::CalendarViewEvent::ReviewCalendarWritePermission
                ) {
                    requested.store(true, Ordering::SeqCst);
                }
            },
        ));
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");
    let _subscription = subscription;

    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(context.cx.update(|cx| {
        calendar.read(cx).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate)
    }));
    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("calendar offers event creation");
    context.simulate_click(new_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("composer exposes the title input");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_input("Synthetic event");
    let submit = context
        .debug_bounds("event-composer-submit")
        .expect("composer offers create action");
    context.simulate_click(submit.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
    })
    .await;
    let review = context
        .debug_bounds("review-calendar-write-permission")
        .expect("denied write exposes an explicit permission review");
    context.simulate_click(review.center(), Modifiers::default());
    context.run_until_parked();

    assert!(permission_review_requested.load(Ordering::SeqCst));
}

#[gpui_kit::test]
async fn native_event_editor_updates_a_cached_timed_appointment(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::event_composer::bind_event_composer_keys);
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    let workflow = Arc::new(CalendarWorkflow::new(
        provider.clone(),
        TestCache::default(),
    ));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let mut calendar_handle = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let calendar = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                super::CalendarWeekSelection {
                    week_start,
                    time_zone: TimeZone::UTC,
                    utc_fallback: true,
                },
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");

    wait_for_ui_condition(context, |_, app| {
        matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| { matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate) })
    );

    select_default_timed_event(context);
    let edit = context
        .debug_bounds("calendar-event-edit")
        .expect("timed calendar event offers an edit action");
    context.simulate_click(edit.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    assert!(context.debug_bounds("event-composer-fields").is_some());

    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("editor exposes the event title");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_keystrokes("ctrl-a");
    context.simulate_input("Updated synthetic appointment");
    draw_calendar_view(context);

    let save = context
        .debug_bounds("event-composer-submit")
        .expect("editor offers a save action");
    let (release, gate) = oneshot::channel();
    provider.hold_event_update_until(gate);
    context.simulate_click(save.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider.event_update_calls.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        provider.event_update_calls.load(Ordering::SeqCst),
        1,
        "saving starts one update request"
    );
    let event_list = context
        .debug_bounds("calendar-event-list")
        .expect("the calendar remains behind the appointment editor");
    context.simulate_click(
        gpui_kit::point(event_list.left() + gpui_kit::px(1.), event_list.center().y),
        Modifiers::default(),
    );
    draw_calendar_view(context);
    assert!(
        context.update(|window, cx| window.has_active_sheet(cx)),
        "an outside click cannot hide a pending update"
    );
    context.simulate_keystrokes("escape");
    draw_calendar_view(context);
    assert!(
        context.update(|window, cx| window.has_active_sheet(cx)),
        "Escape cannot hide a pending update"
    );
    let saving = context
        .debug_bounds("event-composer-submit")
        .expect("pending update keeps its disabled Save action visible");
    context.simulate_click(saving.center(), Modifiers::default());
    assert_eq!(
        provider.event_update_calls.load(Ordering::SeqCst),
        1,
        "a pending update cannot be submitted twice"
    );
    release
        .send(())
        .expect("synthetic provider is waiting on the UI test");
    wait_for_ui_condition(context, |_, app| {
        calendar
            .read(app)
            .events
            .iter()
            .any(|event| event.subject() == Some("Updated synthetic appointment"))
    })
    .await;
    let (updated, saved_status) = context.cx.update(|cx| {
        let view = calendar.read(cx);
        (
            view.events
                .iter()
                .any(|event| event.subject() == Some("Updated synthetic appointment")),
            view.events.len(),
        )
    });
    assert!(
        updated,
        "confirmed edit reloads the updated cached occurrence"
    );
    assert_eq!(
        saved_status, 2,
        "the calendar retains both synthetic events"
    );
    assert!(context.debug_bounds("event-creation-status").is_some());
    assert!(context.debug_bounds("event-composer-close").is_some());
}

#[gpui_kit::test]
async fn native_event_editor_confirms_and_serializes_deletion(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::event_composer::bind_event_composer_keys);
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    let workflow = Arc::new(CalendarWorkflow::new(
        provider.clone(),
        TestCache::default(),
    ));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let mut calendar_handle = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let calendar = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                super::CalendarWeekSelection {
                    week_start,
                    time_zone: TimeZone::UTC,
                    utc_fallback: true,
                },
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");

    wait_for_ui_condition(context, |_, app| {
        matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| { matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate) })
    );

    select_default_timed_event(context);
    let edit = context
        .debug_bounds("calendar-event-edit")
        .expect("timed appointment offers an edit action");
    context.simulate_click(edit.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-actions")
    })
    .await;
    assert!(context.debug_bounds("event-composer-delete").is_some());

    let (release, gate) = oneshot::channel();
    provider.hold_event_deletion_until(gate);
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("editor offers a delete action");
    context.simulate_click(delete.center(), Modifiers::default());
    let cancel = context
        .debug_bounds("event-delete-dialog-cancel")
        .expect("delete opens a confirmation dialog");
    assert!(
        context
            .debug_bounds("event-delete-dialog-confirm")
            .is_some()
    );
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        0,
        "opening confirmation does not start deletion"
    );
    context.simulate_click(cancel.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        0,
        "canceling confirmation leaves the event untouched"
    );

    provider.fail_next_event_deletion(CalendarOperationError::DeleteOutcomeUnknown);
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("cancel returns to the event editor");
    context.simulate_click(delete.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, app| window.has_active_dialog(app)).await;
    let confirm = context
        .debug_bounds("event-delete-dialog-confirm")
        .expect("confirmation offers the destructive action");
    context.simulate_click(confirm.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider.event_deletion_calls.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        1,
        "confirmation starts one delete request"
    );
    context.simulate_keystrokes("escape");
    draw_calendar_view(context);
    assert!(
        context.update(|window, cx| window.has_active_sheet(cx)),
        "Escape cannot hide a pending delete"
    );
    let event_list = context
        .debug_bounds("calendar-event-list")
        .expect("calendar remains behind the editor");
    context.simulate_click(
        gpui_kit::point(event_list.left() + gpui_kit::px(1.), event_list.center().y),
        Modifiers::default(),
    );
    draw_calendar_view(context);
    assert!(
        context.update(|window, cx| window.has_active_sheet(cx)),
        "an outside click cannot hide a pending delete"
    );
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("pending deletion keeps its disabled action visible");
    context.simulate_click(delete.center(), Modifiers::default());
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        1,
        "a pending delete cannot be submitted twice"
    );

    release
        .send(())
        .expect("synthetic provider is waiting on the UI test");
    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness != CalendarWriteReadiness::Ready
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| calendar.read(cx).write_readiness != CalendarWriteReadiness::Ready)
    );
    let done = context
        .debug_bounds("event-composer-close")
        .expect("uncertain delete can be dismissed");
    context.simulate_click(done.center(), Modifiers::default());
    draw_calendar_view(context);

    let sync = context
        .debug_bounds("calendar-sync")
        .expect("calendar offers recovery sync");
    context.simulate_click(sync.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| { calendar.read(cx).write_readiness == CalendarWriteReadiness::Ready })
    );

    let edit = context
        .debug_bounds("calendar-event-edit")
        .expect("successful sync restores event editing");
    context.simulate_click(edit.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-actions")
    })
    .await;
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("reconciled event can be deleted again");
    context.simulate_click(delete.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, app| window.has_active_dialog(app)).await;
    let confirm = context
        .debug_bounds("event-delete-dialog-confirm")
        .expect("reconciled event offers delete confirmation");
    context.simulate_click(confirm.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider.event_deletion_calls.load(Ordering::SeqCst) == 2
    })
    .await;
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        2,
        "a successful sync permits a new explicit delete"
    );
    wait_for_ui_condition(context, |_, app| {
        calendar
            .read(app)
            .events
            .iter()
            .all(|event| event.id().as_str() != "synthetic-timed-event")
    })
    .await;
    let deleted = context.cx.update(|cx| {
        calendar
            .read(cx)
            .events
            .iter()
            .all(|event| event.id().as_str() != "synthetic-timed-event")
    });
    assert!(deleted, "confirmed delete removes the cached occurrence");
    assert!(context.debug_bounds("event-creation-status").is_some());
    assert!(context.debug_bounds("event-composer-close").is_some());
}

#[gpui_kit::test]
async fn unknown_event_update_requires_sync_before_another_write(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::event_composer::bind_event_composer_keys);
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    let workflow = Arc::new(CalendarWorkflow::new(
        provider.clone(),
        TestCache::default(),
    ));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let mut calendar_handle = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let calendar = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                super::CalendarWeekSelection {
                    week_start,
                    time_zone: TimeZone::UTC,
                    utc_fallback: true,
                },
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");

    wait_for_ui_condition(context, |_, app| {
        matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| { matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate) })
    );

    select_default_timed_event(context);
    let edit = context
        .debug_bounds("calendar-event-edit")
        .expect("timed appointment offers an edit action");
    context.simulate_click(edit.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("editor exposes the appointment title");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_keystrokes("ctrl-a");
    context.simulate_input("Uncertain appointment update");
    provider.fail_next_event_update(CalendarOperationError::UpdateOutcomeUnknown);
    let save = context
        .debug_bounds("event-composer-submit")
        .expect("editor offers a save action");
    context.simulate_click(save.center(), Modifiers::default());

    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness != CalendarWriteReadiness::Ready
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| calendar.read(cx).write_readiness != CalendarWriteReadiness::Ready)
    );
    let close = context
        .debug_bounds("event-composer-close")
        .expect("uncertain result can be dismissed");
    context.simulate_click(close.center(), Modifiers::default());
    draw_calendar_view(context);

    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("calendar keeps the new-event action visible during recovery");
    context.simulate_click(new_event.center(), Modifiers::default());
    draw_calendar_view(context);
    assert!(!context.update(|window, cx| window.has_active_sheet(cx)));

    let edit = context
        .debug_bounds("calendar-event-edit")
        .expect("cached appointment remains visible until sync");
    context.simulate_click(edit.center(), Modifiers::default());
    draw_calendar_view(context);
    assert!(!context.update(|window, cx| window.has_active_sheet(cx)));
    assert_eq!(
        provider.event_update_calls.load(Ordering::SeqCst),
        1,
        "disabled Edit starts no second update"
    );

    provider.fail_next_calendar_sync(CalendarOperationError::ServiceUnavailable);
    let sync = context
        .debug_bounds("calendar-sync")
        .expect("calendar exposes a recovery sync action");
    context.simulate_click(sync.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(calendar.read(app).status, super::CalendarStatus::Failed(_))
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| calendar.read(cx).write_readiness != CalendarWriteReadiness::Ready)
    );
    let edit = context
        .debug_bounds("calendar-event-edit")
        .expect("failed sync keeps the appointment visible");
    context.simulate_click(edit.center(), Modifiers::default());
    draw_calendar_view(context);
    assert!(!context.update(|window, cx| window.has_active_sheet(cx)));

    let sync = context
        .debug_bounds("calendar-sync")
        .expect("calendar keeps sync available after failure");
    context.simulate_click(sync.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| { calendar.read(cx).write_readiness == CalendarWriteReadiness::Ready })
    );
    let edit = context
        .debug_bounds("calendar-event-edit")
        .expect("successful sync restores Edit");
    context.simulate_click(edit.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    assert!(context.debug_bounds("event-composer-fields").is_some());
    assert_eq!(
        provider.event_update_calls.load(Ordering::SeqCst),
        1,
        "opening the editor after sync does not replay the update"
    );
}

fn assert_accessible_agenda(context: &mut VisualTestContext, expected_event_count: usize) {
    let (lists, options) = context.update(|window, _| {
        (
            window.find_all_by_role(gpui_kit::Role::ListBox),
            window.find_all_by_role(gpui_kit::Role::ListBoxOption),
        )
    });
    assert_eq!(lists.len(), 1, "the agenda has one listbox");
    assert_eq!(lists[0].label(), Some("Events this week"));
    assert_eq!(options.len(), expected_event_count, "agenda option count");
    assert!(
        options.iter().all(|option| option
            .label()
            .is_some_and(|label| label.contains("Synthetic overnight event"))),
        "every option exposes its event label"
    );
    assert!(
        options
            .iter()
            .all(|option| option.selected() == Some(false))
    );
}

fn accessible_agenda_list_focus(context: &mut VisualTestContext) -> Option<bool> {
    let lists = context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBox));
    assert_eq!(lists.len(), 1, "the agenda has one listbox");
    lists[0].focused()
}

fn assert_accessible_selection(context: &mut VisualTestContext, expected_label: &str) {
    let options =
        context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBoxOption));
    let selected_options: Vec<_> = options
        .iter()
        .filter(|option| option.selected() == Some(true))
        .collect();
    assert_eq!(
        selected_options.len(),
        1,
        "exactly one calendar option is exposed as selected"
    );
    assert_eq!(selected_options[0].label(), Some(expected_label));
}

fn assert_selected_event<S>(
    context: &mut VisualTestContext,
    view: &gpui_kit::Entity<CalendarView<S>>,
    expected: &str,
) where
    S: CalendarSession + ?Sized + 'static,
{
    let selected = context.cx.update(|cx| {
        view.read(cx)
            .selected_agenda_event
            .as_ref()
            .map(|key| key.event_id.as_str().to_owned())
    });
    assert_eq!(selected.as_deref(), Some(expected));
}
