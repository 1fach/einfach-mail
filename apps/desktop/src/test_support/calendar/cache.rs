use std::{
    collections::{HashMap, HashSet},
    future::{Future, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use super::fixtures::timed_event;
use einfach_mail_application::{
    CalendarCache, CalendarCacheError, CalendarEventCreationStage, CalendarSyncMode,
    CalendarSyncState,
};
use einfach_mail_domain::{
    AccountId, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress, CalendarEventCreation,
    CalendarEventUpdate, CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTimePatch,
    CalendarViewReference, EventId, PendingCalendarEventCreation, VersionedCalendarEvent,
};
use time::{Date, Month};

#[derive(Clone, Default)]
pub(crate) struct TestCache {
    events: Option<Vec<CalendarOccurrence>>,
    snapshots: Arc<Mutex<HashMap<CalendarViewReference, Vec<CalendarOccurrence>>>>,
    created_events: Arc<Mutex<HashMap<CalendarViewReference, HashMap<String, CalendarOccurrence>>>>,
    pending_creation: Arc<Mutex<Option<PendingCalendarEventCreation>>>,
    fail_next_rejected_cleanup: Arc<AtomicBool>,
    updated_events: Arc<Mutex<HashMap<CalendarViewReference, HashMap<String, CalendarOccurrence>>>>,
    deleted_event_ids: Arc<Mutex<HashSet<String>>>,
    write_sync_required: Arc<Mutex<HashSet<CalendarViewReference>>>,
    synchronized_views: Arc<Mutex<HashSet<CalendarViewReference>>>,
}

impl TestCache {
    pub(crate) fn with_events(events: Vec<CalendarOccurrence>) -> Self {
        Self {
            events: Some(events),
            ..Self::default()
        }
    }

    pub(crate) fn fail_rejected_cleanup_once(&self) {
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

    fn mark_calendar_write_sync_required(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let views_to_mark = {
            let synchronized_views = self
                .synchronized_views
                .lock()
                .expect("synthetic synchronized view mutex is not poisoned");
            if !synchronized_views.contains(view) {
                return ready(Err(CalendarCacheError::InvalidData));
            }
            synchronized_views
                .iter()
                .filter(|candidate| candidate.account_id() == view.account_id())
                .cloned()
                .collect::<Vec<_>>()
        };
        self.write_sync_required
            .lock()
            .expect("synthetic write recovery mutex is not poisoned")
            .extend(views_to_mark);
        ready(Ok(()))
    }

    fn account_has_calendar_write_sync_required(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<bool, CalendarCacheError>> + Send + '_ {
        ready(Ok(self
            .write_sync_required
            .lock()
            .expect("synthetic write recovery mutex is not poisoned")
            .iter()
            .any(|view| view.account_id() == account_id)))
    }

    fn clear_calendar_write_sync_required(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        self.write_sync_required
            .lock()
            .expect("synthetic write recovery mutex is not poisoned")
            .retain(|view| view.account_id() != account_id);
        ready(Ok(()))
    }

    fn commit_delta_page(
        &self,
        view: &CalendarViewReference,
        page: CalendarDeltaPage,
        completed_at: std::time::SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let (_, progress) = page.into_parts();
        let completed_sync = matches!(&progress, CalendarDeltaProgress::Complete(_));
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
        if completed_sync && state.is_ok() {
            self.synchronized_views
                .lock()
                .expect("synthetic synchronized view mutex is not poisoned")
                .insert(view.clone());
            self.write_sync_required
                .lock()
                .expect("synthetic write recovery mutex is not poisoned")
                .remove(view);
        }
        ready(state)
    }

    fn replace_snapshot(
        &self,
        view: &CalendarViewReference,
        events: Vec<CalendarOccurrence>,
        completed_at: std::time::SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let snapshots = Arc::clone(&self.snapshots);
        let synchronized_views = Arc::clone(&self.synchronized_views);
        let write_sync_required = Arc::clone(&self.write_sync_required);
        let view = view.clone();
        async move {
            let mut event_ids = HashSet::with_capacity(events.len());
            for event in &events {
                if event.reference().account_id() != view.account_id() {
                    return Err(CalendarCacheError::AccountMismatch);
                }
                if event.reference() != &view
                    || !event.overlaps_view()
                    || !event_ids.insert(event.id().as_str())
                {
                    return Err(CalendarCacheError::InvalidData);
                }
            }
            snapshots
                .lock()
                .expect("synthetic snapshot mutex is not poisoned")
                .insert(view.clone(), events);
            synchronized_views
                .lock()
                .expect("synthetic synchronized view mutex is not poisoned")
                .insert(view.clone());
            write_sync_required
                .lock()
                .expect("synthetic write recovery mutex is not poisoned")
                .remove(&view);
            Ok(CalendarSyncState::snapshot(completed_at))
        }
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
        if let Some(events) = self
            .snapshots
            .lock()
            .expect("synthetic snapshot mutex is not poisoned")
            .get(view)
            .cloned()
        {
            return ready(Ok(events));
        }
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
    ) -> impl Future<Output = Result<CalendarEventCreationStage, CalendarCacheError>> + Send + '_
    {
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
                    Ok(CalendarEventCreationStage::ExistingUnresolved)
                }
                None => {
                    *pending = Some(PendingCalendarEventCreation::Unresolved(creation));
                    Ok(CalendarEventCreationStage::NewlyStaged)
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
        let write_sync_required = Arc::clone(&self.write_sync_required);
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
            write_sync_required
                .lock()
                .expect("synthetic write recovery mutex is not poisoned")
                .retain(|view| {
                    view.account_id() != update.source().occurrence().reference().account_id()
                });
            Ok(())
        }
    }

    fn commit_event_deletion(
        &self,
        event: &VersionedCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let deleted_event_ids = Arc::clone(&self.deleted_event_ids);
        let write_sync_required = Arc::clone(&self.write_sync_required);
        let account_id = event.occurrence().reference().account_id().clone();
        let event_id = event.occurrence().id().as_str().to_owned();
        async move {
            deleted_event_ids
                .lock()
                .expect("synthetic deleted event set is not poisoned")
                .insert(event_id);
            write_sync_required
                .lock()
                .expect("synthetic write recovery mutex is not poisoned")
                .retain(|view| view.account_id() != &account_id);
            Ok(())
        }
    }
}
