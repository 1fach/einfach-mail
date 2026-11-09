use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use einfach_mail_domain::{
    AccountId, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarEventCreation, CalendarEventCreationId, CalendarEventUpdate, CalendarEventUpdateDraft,
    CalendarEventVersion, CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference, EditableCalendarEvent,
    EventId, PendingCalendarEventCreation,
};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

use crate::{
    CalendarDeltaPageResponse, CalendarSyncMode, account_recovery::AccountWorkflowRecovery,
    operation_reservations::ActiveOperations,
};

use super::{
    CalendarCache, CalendarCacheError, CalendarDeltaRequest, CalendarDeltaResponse,
    CalendarEventCreationOutcome, CalendarEventDeletionOutcome, CalendarEventUpdateOutcome,
    CalendarOperationError, CalendarProvider, CalendarSyncState, CalendarWorkflow,
    CalendarWorkflowError, CalendarWriteReadiness,
};

struct CalendarWriteGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

#[derive(Clone, Default)]
struct FixtureProvider {
    pages: Arc<Mutex<VecDeque<Result<CalendarDeltaResponse, CalendarOperationError>>>>,
    delta_gate: Arc<Mutex<Option<CalendarWriteGate>>>,
    requests: Arc<Mutex<Vec<CalendarDeltaRequest>>>,
    creations: Arc<Mutex<Vec<CalendarEventCreationId>>>,
    creation_error: Arc<Mutex<Option<CalendarOperationError>>>,
    creation_override: Arc<Mutex<Option<CalendarOccurrence>>>,
    update_calls: Arc<Mutex<Vec<CalendarEventUpdate>>>,
    update_gate: Arc<Mutex<Option<CalendarWriteGate>>>,
    update_error: Arc<Mutex<Option<CalendarOperationError>>>,
    deletion_calls: Arc<Mutex<usize>>,
    deletion_gate: Arc<Mutex<Option<CalendarWriteGate>>>,
    deletion_error: Arc<Mutex<Option<CalendarOperationError>>>,
}

impl FixtureProvider {
    fn with_pages(
        pages: impl IntoIterator<Item = Result<CalendarDeltaResponse, CalendarOperationError>>,
    ) -> Self {
        Self {
            pages: Arc::new(Mutex::new(pages.into_iter().collect())),
            delta_gate: Arc::new(Mutex::new(None)),
            requests: Arc::new(Mutex::new(Vec::new())),
            creations: Arc::new(Mutex::new(Vec::new())),
            creation_error: Arc::new(Mutex::new(None)),
            creation_override: Arc::new(Mutex::new(None)),
            update_calls: Arc::new(Mutex::new(Vec::new())),
            update_gate: Arc::new(Mutex::new(None)),
            update_error: Arc::new(Mutex::new(None)),
            deletion_calls: Arc::new(Mutex::new(0)),
            deletion_gate: Arc::new(Mutex::new(None)),
            deletion_error: Arc::new(Mutex::new(None)),
        }
    }

    fn push_page(&self, page: Result<CalendarDeltaResponse, CalendarOperationError>) {
        self.pages
            .lock()
            .expect("synthetic page queue mutex is not poisoned")
            .push_back(page);
    }

    fn hold_next_delta(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self
            .delta_gate
            .lock()
            .expect("synthetic delta gate mutex is not poisoned") = Some(CalendarWriteGate {
            started: started_tx,
            release: release_rx,
        });
        (started_rx, release_tx)
    }

    fn fail_next_creation(&self, error: CalendarOperationError) {
        *self
            .creation_error
            .lock()
            .expect("synthetic creation error mutex is not poisoned") = Some(error);
    }

    fn return_creation(&self, event: CalendarOccurrence) {
        *self
            .creation_override
            .lock()
            .expect("synthetic creation result mutex is not poisoned") = Some(event);
    }

    fn fail_next_update(&self, error: CalendarOperationError) {
        *self
            .update_error
            .lock()
            .expect("synthetic update error mutex is not poisoned") = Some(error);
    }

    fn hold_next_update(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self
            .update_gate
            .lock()
            .expect("synthetic update gate mutex is not poisoned") = Some(CalendarWriteGate {
            started: started_tx,
            release: release_rx,
        });
        (started_rx, release_tx)
    }

    fn fail_next_deletion(&self, error: CalendarOperationError) {
        *self
            .deletion_error
            .lock()
            .expect("synthetic deletion error mutex is not poisoned") = Some(error);
    }

    fn hold_next_deletion(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self
            .deletion_gate
            .lock()
            .expect("synthetic deletion gate mutex is not poisoned") = Some(CalendarWriteGate {
            started: started_tx,
            release: release_rx,
        });
        (started_rx, release_tx)
    }
}

impl CalendarProvider for FixtureProvider {
    fn calendar_delta_page(
        &self,
        _view: &CalendarViewReference,
        request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        let gate = self
            .delta_gate
            .lock()
            .expect("synthetic delta gate mutex is not poisoned")
            .take();
        let pages = Arc::clone(&self.pages);
        let requests = Arc::clone(&self.requests);
        async move {
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .push(request);
            if let Some(gate) = gate {
                gate.started
                    .send(())
                    .expect("the calendar sync test is waiting at its delta gate");
                gate.release
                    .await
                    .expect("the calendar sync test releases its delta gate");
            }
            pages
                .lock()
                .expect("synthetic page queue mutex is not poisoned")
                .pop_front()
                .unwrap_or(Err(CalendarOperationError::ServiceUnavailable))
        }
    }

    fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let creation = creation.clone();
        let creations = Arc::clone(&self.creations);
        let creation_error = Arc::clone(&self.creation_error);
        let creation_override = Arc::clone(&self.creation_override);
        async move {
            creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .push(creation.id());
            if let Some(error) = creation_error
                .lock()
                .expect("synthetic creation error mutex is not poisoned")
                .take()
            {
                return Err(error);
            }
            if let Some(event) = creation_override
                .lock()
                .expect("synthetic creation result mutex is not poisoned")
                .take()
            {
                return Ok(event);
            }
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
            let id = EventId::try_from(format!("synthetic-created-{}", creation.id().as_uuid()))
                .expect("synthetic created event identifier is valid");
            patch
                .into_new(creation.view().clone(), id)
                .map_err(|_| CalendarOperationError::InvalidResponse)
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
        self.update_calls
            .lock()
            .expect("synthetic update log mutex is not poisoned")
            .push(update.clone());
        let gate = self
            .update_gate
            .lock()
            .expect("synthetic update gate mutex is not poisoned")
            .take();
        let error = self
            .update_error
            .lock()
            .expect("synthetic update error mutex is not poisoned")
            .take();
        async move {
            if let Some(gate) = gate {
                gate.started
                    .send(())
                    .expect("the calendar update test is waiting at its provider gate");
                gate.release
                    .await
                    .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
            }
            if let Some(error) = error {
                return Err(error);
            }
            updated_event(&update)
        }
    }

    fn delete_event(
        &self,
        _event: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        let deletion_calls = Arc::clone(&self.deletion_calls);
        let gate = self
            .deletion_gate
            .lock()
            .expect("synthetic deletion gate mutex is not poisoned")
            .take();
        let error = self
            .deletion_error
            .lock()
            .expect("synthetic deletion error mutex is not poisoned")
            .take();
        async move {
            *deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned") += 1;
            if let Some(gate) = gate {
                gate.started
                    .send(())
                    .expect("the calendar delete test is waiting at its provider gate");
                gate.release
                    .await
                    .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
            }
            error.map_or(Ok(()), Err)
        }
    }
}

#[derive(Clone, Default)]
struct FixtureCache {
    views: Arc<Mutex<HashMap<CalendarViewReference, CachedView>>>,
    pending: Arc<Mutex<HashMap<AccountId, PendingCalendarEventCreation>>>,
    creation_commit_error: Arc<Mutex<Option<CalendarCacheError>>>,
    rejection_mark_error: Arc<Mutex<Option<CalendarCacheError>>>,
    rejection_cleanup_error: Arc<Mutex<Option<CalendarCacheError>>>,
    update_commit_error: Arc<Mutex<Option<CalendarCacheError>>>,
    deletion_commit_error: Arc<Mutex<Option<CalendarCacheError>>>,
    write_recovery_cleanup_error: Arc<Mutex<Option<CalendarCacheError>>>,
}

impl FixtureCache {
    fn fail_creation_commit(&self, error: CalendarCacheError) {
        *self
            .creation_commit_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    fn fail_rejection_cleanup(&self, error: CalendarCacheError) {
        *self
            .rejection_cleanup_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    fn fail_rejection_mark(&self, error: CalendarCacheError) {
        *self
            .rejection_mark_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    fn fail_update_commit(&self, error: CalendarCacheError) {
        *self
            .update_commit_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    fn fail_deletion_commit(&self, error: CalendarCacheError) {
        *self
            .deletion_commit_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    fn fail_write_recovery_cleanup(&self, error: CalendarCacheError) {
        *self
            .write_recovery_cleanup_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }
}

#[derive(Clone)]
struct CachedView {
    state: CalendarSyncState,
    events: HashMap<String, CalendarOccurrence>,
    replacement_events: Option<HashMap<String, CalendarOccurrence>>,
    removed_in_replacement: std::collections::HashSet<String>,
    write_sync_required: bool,
}

impl CachedView {
    fn empty() -> Self {
        Self {
            state: CalendarSyncState::initial(),
            events: HashMap::new(),
            replacement_events: None,
            removed_in_replacement: std::collections::HashSet::new(),
            write_sync_required: false,
        }
    }
}

impl CalendarCache for FixtureCache {
    fn ensure_view(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let view = view.clone();
        async move {
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            Ok(views
                .entry(view)
                .or_insert_with(CachedView::empty)
                .state
                .clone())
        }
    }

    fn account_has_active_replacement(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<bool, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let account_id = account_id.clone();
        async move {
            Ok(views
                .lock()
                .expect("synthetic cache mutex is not poisoned")
                .iter()
                .any(|(view, cached)| {
                    view.account_id() == &account_id && cached.state.replacement_active()
                }))
        }
    }

    fn mark_calendar_write_sync_required(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let view = view.clone();
        async move {
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            if !views
                .get(&view)
                .is_some_and(|cached| cached.state.last_synchronized_at().is_some())
            {
                return Err(CalendarCacheError::InvalidData);
            }
            for (candidate, cached) in &mut *views {
                if candidate.account_id() == view.account_id()
                    && cached.state.last_synchronized_at().is_some()
                {
                    cached.write_sync_required = true;
                }
            }
            Ok(())
        }
    }

    fn account_has_calendar_write_sync_required(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<bool, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let account_id = account_id.clone();
        async move {
            Ok(views
                .lock()
                .expect("synthetic cache mutex is not poisoned")
                .iter()
                .any(|(view, cached)| {
                    view.account_id() == &account_id && cached.write_sync_required
                }))
        }
    }

    fn clear_calendar_write_sync_required(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let cleanup_error = Arc::clone(&self.write_recovery_cleanup_error);
        let account_id = account_id.clone();
        async move {
            if let Some(error) = cleanup_error
                .lock()
                .expect("synthetic cache error mutex is not poisoned")
                .take()
            {
                return Err(error);
            }
            for (view, cached) in &mut *views.lock().expect("synthetic cache mutex is not poisoned")
            {
                if view.account_id() == &account_id {
                    cached.write_sync_required = false;
                }
            }
            Ok(())
        }
    }

    fn commit_delta_page(
        &self,
        view: &CalendarViewReference,
        page: CalendarDeltaPage,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let view = view.clone();
        async move {
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            let cached = views.entry(view.clone()).or_insert_with(CachedView::empty);
            let mut staged = cached.clone();
            let (changes, progress) = page.into_parts();
            let complete_sync = matches!(&progress, CalendarDeltaProgress::Complete(_));
            if staged.state.replacement_active() {
                apply_replacement_changes(&mut staged, &view, changes)?;
                match progress {
                    CalendarDeltaProgress::Next(link) => {
                        staged.state = CalendarSyncState::from_checkpoint(
                            staged.state.delta_link().cloned(),
                            Some(link),
                            staged.state.last_synchronized_at(),
                            CalendarSyncMode::Replacement,
                        )?;
                    }
                    CalendarDeltaProgress::Complete(link) => {
                        staged.events = staged
                            .replacement_events
                            .take()
                            .ok_or(CalendarCacheError::InvalidData)?
                            .into_iter()
                            .filter(|(_, event)| event.overlaps_view())
                            .collect();
                        staged.removed_in_replacement.clear();
                        staged.state = CalendarSyncState::from_checkpoint(
                            Some(link),
                            None,
                            Some(completed_at),
                            CalendarSyncMode::Incremental,
                        )?;
                    }
                }
            } else {
                apply_incremental_changes(&mut staged.events, &view, changes)?;
                staged.state = match progress {
                    CalendarDeltaProgress::Next(link) => CalendarSyncState::from_checkpoint(
                        staged.state.delta_link().cloned(),
                        Some(link),
                        staged.state.last_synchronized_at(),
                        CalendarSyncMode::Incremental,
                    )?,
                    CalendarDeltaProgress::Complete(link) => CalendarSyncState::from_checkpoint(
                        Some(link),
                        None,
                        Some(completed_at),
                        CalendarSyncMode::Incremental,
                    )?,
                };
            }
            if complete_sync {
                staged.write_sync_required = false;
            }
            let state = staged.state.clone();
            *cached = staged;
            Ok(state)
        }
    }

    fn replace_snapshot(
        &self,
        view: &CalendarViewReference,
        events: Vec<CalendarOccurrence>,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
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

            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            let cached = views.entry(view).or_insert_with(CachedView::empty);
            cached.events = events
                .into_iter()
                .map(|event| (event.id().as_str().to_owned(), event))
                .collect();
            cached.replacement_events = None;
            cached.removed_in_replacement.clear();
            cached.state = CalendarSyncState::snapshot(completed_at);
            cached.write_sync_required = false;
            Ok(cached.state.clone())
        }
    }

    fn begin_full_resync(
        &self,
        view: &CalendarViewReference,
        reset_link: Option<CalendarDeltaLink>,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let view = view.clone();
        async move {
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            let cached = views.entry(view).or_insert_with(CachedView::empty);
            cached.replacement_events = Some(HashMap::new());
            cached.removed_in_replacement.clear();
            cached.state = CalendarSyncState::from_checkpoint(
                cached.state.delta_link().cloned(),
                reset_link,
                cached.state.last_synchronized_at(),
                CalendarSyncMode::Replacement,
            )?;
            Ok(cached.state.clone())
        }
    }

    fn list_events(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<Vec<CalendarOccurrence>, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let view = view.clone();
        async move {
            let views = views.lock().expect("synthetic cache mutex is not poisoned");
            Ok(views
                .get(&view)
                .map(|cached| cached.events.values().cloned().collect())
                .unwrap_or_default())
        }
    }

    fn stage_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let pending = Arc::clone(&self.pending);
        let creation = creation.clone();
        async move {
            let mut pending = pending
                .lock()
                .expect("synthetic cache mutex is not poisoned");
            match pending.get(creation.view().account_id()) {
                Some(PendingCalendarEventCreation::Unresolved(existing))
                    if existing == &creation =>
                {
                    Ok(())
                }
                Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
                None => {
                    pending.insert(
                        creation.view().account_id().clone(),
                        PendingCalendarEventCreation::Unresolved(creation),
                    );
                    Ok(())
                }
            }
        }
    }

    fn pending_event_creation(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Option<PendingCalendarEventCreation>, CalendarCacheError>> + Send + '_
    {
        let pending = Arc::clone(&self.pending);
        let account_id = account_id.clone();
        async move {
            Ok(pending
                .lock()
                .expect("synthetic cache mutex is not poisoned")
                .get(&account_id)
                .cloned())
        }
    }

    fn mark_event_creation_rejected(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let pending = Arc::clone(&self.pending);
        let rejection_mark_error = Arc::clone(&self.rejection_mark_error);
        let creation = creation.clone();
        async move {
            if let Some(error) = rejection_mark_error
                .lock()
                .expect("synthetic cache error mutex is not poisoned")
                .take()
            {
                return Err(error);
            }
            let mut pending = pending
                .lock()
                .expect("synthetic cache mutex is not poisoned");
            match pending.get(creation.view().account_id()) {
                None => Ok(()),
                Some(existing) if existing.creation() == &creation => {
                    pending.insert(
                        creation.view().account_id().clone(),
                        PendingCalendarEventCreation::Rejected(creation),
                    );
                    Ok(())
                }
                Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
            }
        }
    }

    fn discard_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let pending = Arc::clone(&self.pending);
        let rejection_cleanup_error = Arc::clone(&self.rejection_cleanup_error);
        let creation = creation.clone();
        async move {
            if let Some(error) = rejection_cleanup_error
                .lock()
                .expect("synthetic cache error mutex is not poisoned")
                .take()
            {
                return Err(error);
            }
            let mut pending = pending
                .lock()
                .expect("synthetic cache mutex is not poisoned");
            match pending.get(creation.view().account_id()) {
                None => Ok(()),
                Some(PendingCalendarEventCreation::Rejected(existing)) if existing == &creation => {
                    pending.remove(creation.view().account_id());
                    Ok(())
                }
                Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
            }
        }
    }

    fn commit_event_creation(
        &self,
        creation: &CalendarEventCreation,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let pending = Arc::clone(&self.pending);
        let views = Arc::clone(&self.views);
        let creation_commit_error = Arc::clone(&self.creation_commit_error);
        let creation = creation.clone();
        let event = event.clone();
        async move {
            if let Some(error) = creation_commit_error
                .lock()
                .expect("synthetic cache error mutex is not poisoned")
                .take()
            {
                return Err(error);
            }
            let mut pending = pending
                .lock()
                .expect("synthetic cache mutex is not poisoned");
            if pending.get(creation.view().account_id())
                != Some(&PendingCalendarEventCreation::Unresolved(creation.clone()))
                || event.reference() != creation.view()
                || event.subject() != Some(creation.draft().subject())
                || event.time() != creation.draft().time()
            {
                return Err(CalendarCacheError::InvalidData);
            }
            views
                .lock()
                .expect("synthetic cache mutex is not poisoned")
                .entry(creation.view().clone())
                .or_insert_with(CachedView::empty)
                .events
                .insert(event.id().as_str().to_owned(), event);
            pending.remove(creation.view().account_id());
            Ok(())
        }
    }

    fn commit_event_update(
        &self,
        update: &CalendarEventUpdate,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let update = update.clone();
        let event = event.clone();
        let update_commit_error = Arc::clone(&self.update_commit_error);
        async move {
            if let Some(error) = update_commit_error
                .lock()
                .expect("synthetic cache error mutex is not poisoned")
                .take()
            {
                return Err(error);
            }
            if !update.confirms(&event) {
                return Err(CalendarCacheError::InvalidData);
            }
            let reference = update.source().occurrence().reference().clone();
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            for (view, cached) in views.iter_mut() {
                if view.account_id() == reference.account_id() {
                    cached.events.remove(event.id().as_str());
                    cached.write_sync_required = false;
                }
            }
            if event.time().overlaps(reference.range()) {
                views
                    .entry(reference)
                    .or_insert_with(CachedView::empty)
                    .events
                    .insert(event.id().as_str().to_owned(), event);
            }
            Ok(())
        }
    }

    fn commit_event_deletion(
        &self,
        event: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let source_view = event.occurrence().reference().clone();
        let event_id = event.occurrence().id().as_str().to_owned();
        let deletion_commit_error = Arc::clone(&self.deletion_commit_error);
        async move {
            if let Some(error) = deletion_commit_error
                .lock()
                .expect("synthetic cache error mutex is not poisoned")
                .take()
            {
                return Err(error);
            }
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            if !views.contains_key(&source_view) {
                return Err(CalendarCacheError::InvalidData);
            }
            for (view, cached) in views.iter_mut() {
                if view.account_id() == source_view.account_id() {
                    cached.events.remove(&event_id);
                    cached.write_sync_required = false;
                }
            }
            Ok(())
        }
    }
}

fn creation(view: CalendarViewReference) -> CalendarEventCreation {
    let local_start = PrimitiveDateTime::new(
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic date is valid"),
        Time::from_hms(9, 30, 0).expect("synthetic time is valid"),
    );
    let local_end = PrimitiveDateTime::new(
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic date is valid"),
        Time::from_hms(10, 0, 0).expect("synthetic time is valid"),
    );
    let draft = einfach_mail_domain::CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        local_start,
        local_end,
        "Etc/UTC"
            .to_owned()
            .try_into()
            .expect("synthetic time zone is valid"),
        utc(2030, Month::January, 8) + time::Duration::hours(9) + time::Duration::minutes(30),
        utc(2030, Month::January, 8) + time::Duration::hours(10),
    )
    .expect("synthetic event draft is valid");
    CalendarEventCreation::try_new(view, draft)
        .expect("synthetic event endpoints fit the selected week")
}

fn mismatched_creation_event(creation: &CalendarEventCreation) -> CalendarOccurrence {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        panic!("synthetic creation fixture uses a timed draft");
    };
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Different synthetic subject".to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(*start);
    time.set_end(*end);
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
        .into_new(
            creation.view().clone(),
            EventId::try_from("synthetic-mismatched-event".to_owned())
                .expect("synthetic event ID is valid"),
        )
        .expect("synthetic mismatched event has a valid time")
}

fn updated_event(
    update: &CalendarEventUpdate,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let source = update.source().occurrence();
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(update.draft().subject().to_owned()));
    let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
        return Err(CalendarOperationError::InvalidResponse);
    };
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(*start);
    time.set_end(*end);
    time.set_is_all_day(false);
    patch.set_time(time);
    let mut event = source.clone();
    patch
        .apply_to(&mut event)
        .map_err(|_| CalendarOperationError::InvalidResponse)?;
    Ok(event)
}

fn update_request(event: EditableCalendarEvent) -> CalendarEventUpdate {
    let draft = CalendarEventUpdateDraft::timed(
        "Updated planning".to_owned(),
        utc(2030, Month::January, 8) + time::Duration::hours(11),
        utc(2030, Month::January, 8) + time::Duration::hours(12),
    )
    .expect("synthetic edit draft is valid");
    CalendarEventUpdate::new(event, draft).expect("synthetic event values changed")
}

struct CalendarWorkflowPair {
    first: Arc<CalendarWorkflow<FixtureProvider, FixtureCache>>,
    sibling: Arc<CalendarWorkflow<FixtureProvider, FixtureCache>>,
}

fn workflows_sharing_account_state(
    provider: &FixtureProvider,
    cache: &FixtureCache,
) -> CalendarWorkflowPair {
    let reservations = Arc::new(Mutex::new(ActiveOperations::default()));
    let recovery = Arc::new(AccountWorkflowRecovery::default());
    let first = Arc::new(CalendarWorkflow::with_reservations(
        workflow_account_id(),
        provider.clone(),
        cache.clone(),
        Arc::clone(&reservations),
        Arc::clone(&recovery),
    ));
    let second = Arc::new(CalendarWorkflow::with_reservations(
        workflow_account_id(),
        provider.clone(),
        cache.clone(),
        reservations,
        recovery,
    ));
    CalendarWorkflowPair {
        first,
        sibling: second,
    }
}

fn apply_incremental_changes(
    events: &mut HashMap<String, CalendarOccurrence>,
    view: &CalendarViewReference,
    changes: Vec<CalendarDeltaChange>,
) -> Result<(), CalendarCacheError> {
    for change in changes {
        match change {
            CalendarDeltaChange::Removed { id } => {
                events.remove(id.as_str());
            }
            CalendarDeltaChange::Upsert { id, patch } => {
                let occurrence =
                    updated_occurrence(view, id.clone(), patch, events.remove(id.as_str()))?;
                if occurrence.overlaps_view() {
                    events.insert(id.as_str().to_owned(), occurrence);
                }
            }
        }
    }
    Ok(())
}

fn apply_replacement_changes(
    cached: &mut CachedView,
    view: &CalendarViewReference,
    changes: Vec<CalendarDeltaChange>,
) -> Result<(), CalendarCacheError> {
    let CachedView {
        events,
        replacement_events,
        removed_in_replacement,
        ..
    } = cached;
    let replacement_events = replacement_events
        .as_mut()
        .ok_or(CalendarCacheError::InvalidData)?;
    for change in changes {
        match change {
            CalendarDeltaChange::Removed { id } => {
                replacement_events.remove(id.as_str());
                removed_in_replacement.insert(id.as_str().to_owned());
            }
            CalendarDeltaChange::Upsert { id, patch } => {
                let existing = replacement_events.remove(id.as_str()).or_else(|| {
                    (!removed_in_replacement.contains(id.as_str()))
                        .then(|| events.get(id.as_str()).cloned())
                        .flatten()
                });
                let occurrence = updated_occurrence(view, id.clone(), patch, existing)?;
                removed_in_replacement.remove(id.as_str());
                replacement_events.insert(id.as_str().to_owned(), occurrence);
            }
        }
    }
    Ok(())
}

fn updated_occurrence(
    view: &CalendarViewReference,
    id: EventId,
    patch: CalendarOccurrencePatch,
    existing: Option<CalendarOccurrence>,
) -> Result<CalendarOccurrence, CalendarCacheError> {
    match existing {
        Some(mut occurrence) => {
            patch
                .apply_to(&mut occurrence)
                .map_err(|_| CalendarCacheError::InvalidData)?;
            Ok(occurrence)
        }
        None => patch
            .into_new(view.clone(), id)
            .map_err(|_| CalendarCacheError::InvalidData),
    }
}

fn view() -> CalendarViewReference {
    let account = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account identifier is valid");
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
    .expect("synthetic week range is valid");
    CalendarViewReference::new(account, range)
}

fn workflow_account_id() -> AccountId {
    view().account_id().clone()
}

fn event_id() -> EventId {
    EventId::try_from("synthetic-event".to_owned()).expect("synthetic event ID is valid")
}

fn replacement_event_id() -> EventId {
    EventId::try_from("synthetic-replacement-event".to_owned())
        .expect("synthetic replacement event ID is valid")
}

fn all_day_patch(subject: Option<&str>, location: Option<&str>) -> CalendarOccurrencePatch {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(subject.map(str::to_owned));
    patch.set_location(location.map(str::to_owned));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(utc(2030, Month::January, 8));
    time.set_end(utc(2030, Month::January, 10));
    time.set_is_all_day(true);
    patch.set_time(time);
    patch
}

#[test]
fn snapshot_cache_replaces_events_and_rejects_duplicate_ids() {
    smol::block_on(async {
        let view = view();
        let patch = all_day_patch(Some("Synthetic snapshot"), None);
        let occurrence = patch
            .into_new(view.clone(), replacement_event_id())
            .expect("synthetic snapshot event is complete");
        let cache = FixtureCache::default();
        let completed_at = UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);

        let state = cache
            .replace_snapshot(&view, vec![occurrence.clone()], completed_at)
            .await
            .expect("complete snapshot replaces cached events");
        assert!(state.is_snapshot());
        assert!(state.delta_link().is_none());
        assert_eq!(state.last_synchronized_at(), Some(completed_at));
        assert_eq!(
            cache
                .list_events(&view)
                .await
                .expect("snapshot is readable"),
            vec![occurrence.clone()]
        );

        assert_eq!(
            cache
                .replace_snapshot(&view, vec![occurrence.clone(), occurrence], completed_at)
                .await,
            Err(CalendarCacheError::InvalidData)
        );
        assert_eq!(
            cache
                .list_events(&view)
                .await
                .expect("failed snapshot keeps cache"),
            vec![
                all_day_patch(Some("Synthetic snapshot"), None)
                    .into_new(view.clone(), replacement_event_id())
                    .expect("synthetic snapshot event remains complete")
            ]
        );

        cache
            .replace_snapshot(&view, Vec::new(), completed_at)
            .await
            .expect("an empty complete snapshot clears its view");
        assert!(
            cache
                .list_events(&view)
                .await
                .expect("empty snapshot is readable")
                .is_empty()
        );
    });
}

fn utc(year: i32, month: Month, day: u8) -> OffsetDateTime {
    Date::from_calendar_date(year, month, day)
        .expect("synthetic date is valid")
        .midnight()
        .assume_utc()
}

fn link(cursor_token: &str) -> CalendarDeltaLink {
    CalendarDeltaLink::from_opaque(format!(
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken={cursor_token}"
    ))
}

fn response(
    changes: Vec<CalendarDeltaChange>,
    progress: CalendarDeltaProgress,
) -> CalendarDeltaResponse {
    CalendarDeltaResponse::Page(CalendarDeltaPageResponse::new(
        CalendarDeltaPage::new(changes, progress),
        128,
    ))
}

async fn synchronize_empty_view(
    workflow: &CalendarWorkflow<FixtureProvider, FixtureCache>,
    provider: &FixtureProvider,
    view: &CalendarViewReference,
    cursor: &str,
) {
    provider.push_page(Ok(response(
        Vec::new(),
        CalendarDeltaProgress::Complete(link(cursor)),
    )));
    workflow
        .sync_view(view)
        .await
        .expect("the synthetic calendar view completes its sync");
}

fn cross_week_timed_event_patch() -> CalendarOccurrencePatch {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Cross-week appointment".to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(utc(2030, Month::January, 13) + time::Duration::hours(9));
    time.set_end(utc(2030, Month::January, 15) + time::Duration::hours(10));
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
}

#[test]
fn sync_commits_pages_and_preserves_omitted_fields() {
    smol::block_on(async {
        let view = view();
        let next_link = link("synthetic-next");
        let delta_link = link("synthetic-complete");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: all_day_patch(Some("Synthetic appointment"), Some("Room 1")),
                }],
                CalendarDeltaProgress::Next(next_link.clone()),
            )),
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: {
                        let mut patch = CalendarOccurrencePatch::new();
                        patch.set_location(None);
                        let mut time = CalendarOccurrenceTimePatch::new();
                        time.set_start(utc(2030, Month::January, 9));
                        patch.set_time(time);
                        patch
                    },
                }],
                CalendarDeltaProgress::Complete(delta_link.clone()),
            )),
        ]);
        let request_log = Arc::clone(&provider.requests);
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider, FixtureCache::default());

        let report = workflow.sync_view(&view).await.expect("both pages commit");
        assert_eq!(report.page_count(), 2);
        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(next_link)
            ]
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic snapshot is readable");
        assert_eq!(snapshot.sync_state().delta_link(), Some(&delta_link));
        assert!(snapshot.sync_state().next_link().is_none());
        assert_eq!(snapshot.events().len(), 1);
        assert_eq!(
            snapshot.events()[0].subject(),
            Some("Synthetic appointment")
        );
        assert_eq!(snapshot.events()[0].location(), None);
        assert!(matches!(
            snapshot.events()[0].time(),
            CalendarOccurrenceTime::AllDay { start, end_exclusive }
                if *start == Date::from_calendar_date(2030, Month::January, 9).expect("valid date")
                    && *end_exclusive == Date::from_calendar_date(2030, Month::January, 10).expect("valid date")
        ));
    });
}

#[test]
fn interrupted_sync_resumes_the_saved_next_link() {
    smol::block_on(async {
        let view = view();
        let next_link = link("synthetic-resume");
        let delta_link = link("synthetic-final");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: all_day_patch(Some("Synthetic appointment"), None),
                }],
                CalendarDeltaProgress::Next(next_link.clone()),
            )),
            Err(CalendarOperationError::ServiceUnavailable),
        ]);
        let request_log = Arc::clone(&provider.requests);
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(delta_link.clone()),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("the interrupted cursor resumes");

        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(next_link.clone()),
                CalendarDeltaRequest::Continue(next_link),
            ]
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic snapshot is readable");
        assert_eq!(snapshot.sync_state().delta_link(), Some(&delta_link));
        assert!(snapshot.sync_state().next_link().is_none());
        assert_eq!(snapshot.events().len(), 1);
    });
}

#[test]
fn repeated_delta_cursor_stops_before_committing_the_looping_page() {
    smol::block_on(async {
        let view = view();
        let repeated_link = link("synthetic-repeated");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                Vec::new(),
                CalendarDeltaProgress::Next(repeated_link.clone()),
            )),
            Ok(response(
                Vec::new(),
                CalendarDeltaProgress::Next(repeated_link),
            )),
        ]);
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider, FixtureCache::default());

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::RepeatedCursor)
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic cursor state is readable");
        assert!(snapshot.sync_state().next_link().is_some());
    });
}

#[test]
fn a_completed_cursor_remains_available_for_the_next_round() {
    smol::block_on(async {
        let view = view();
        let delta_link = link("synthetic-delta");
        let provider = FixtureProvider::with_pages([Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(delta_link.clone()),
        ))]);
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        workflow
            .sync_view(&view)
            .await
            .expect("initial round completes");
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("synthetic-next-round")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("following round completes");

        assert_eq!(
            *provider
                .requests
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(delta_link)
            ]
        );
    });
}

#[test]
fn event_creation_waits_for_a_successful_view_sync() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the unsynchronized view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::ViewSyncRequired
        );
        assert_eq!(
            workflow.create_event(&creation).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("the unsynchronized view remains readable");
        assert!(snapshot.pending_creation().is_none());
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );

        synchronize_empty_view(&workflow, &provider, &view, "creation-initial-sync").await;
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the synchronized view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::Ready
        );
        assert!(matches!(
            workflow.create_event(&creation).await,
            Ok(CalendarEventCreationOutcome::Cached(_))
        ));
    });
}

#[test]
fn event_creation_reuses_its_pending_id_after_an_uncertain_provider_failure() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.fail_next_creation(CalendarOperationError::ServiceUnavailable);
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "create-retry-initial-sync").await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        let pending = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic pending operation can be read");
        assert_eq!(
            pending.pending_creation().map(|item| item.creation().id()),
            Some(creation.id())
        );
        assert!(pending.events().is_empty());

        let outcome = workflow
            .create_event(&creation)
            .await
            .expect("same operation confirms on retry");
        assert!(matches!(outcome, CalendarEventCreationOutcome::Cached(_)));
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id(), creation.id()]
        );
        let saved = workflow
            .load_cached_view(&view)
            .await
            .expect("confirmed event is readable from the synthetic cache");
        assert!(saved.pending_creation().is_none());
        assert_eq!(saved.events().len(), 1);
        assert_eq!(
            saved.events()[0].subject(),
            Some("Synthetic planning meeting")
        );
    });
}

#[test]
fn event_creation_recovers_when_cache_commit_fails_after_graph_confirmation() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        cache.fail_creation_commit(CalendarCacheError::Unavailable);
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(&workflow, &provider, &view, "create-cache-initial-sync").await;

        let first_outcome = workflow
            .create_event(&creation)
            .await
            .expect("Graph confirmation is reported even when local commit fails");
        assert!(matches!(
            first_outcome,
            CalendarEventCreationOutcome::ConfirmedButNotCached {
                cache_error: CalendarCacheError::Unavailable,
                ..
            }
        ));
        let pending = workflow
            .load_cached_view(&view)
            .await
            .expect("the confirmed operation remains available for recovery");
        assert_eq!(
            pending
                .pending_creation()
                .map(|pending| pending.creation().id()),
            Some(creation.id())
        );
        assert!(pending.events().is_empty());

        let retry_outcome = workflow
            .create_event(&creation)
            .await
            .expect("retry confirms and saves the same operation");
        assert!(matches!(
            retry_outcome,
            CalendarEventCreationOutcome::Cached(_)
        ));
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id(), creation.id()]
        );
        let saved = workflow
            .load_cached_view(&view)
            .await
            .expect("saved event is readable from the synthetic cache");
        assert!(saved.pending_creation().is_none());
        assert_eq!(saved.events().len(), 1);
    });
}

#[test]
fn event_creation_rejects_an_unrelated_pending_operation_for_the_account() {
    smol::block_on(async {
        let view = view();
        let pending_creation = creation(view.clone());
        let new_creation = creation(view);
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(
            &workflow,
            &provider,
            pending_creation.view(),
            "create-pending-initial-sync",
        )
        .await;
        cache
            .stage_event_creation(&pending_creation)
            .await
            .expect("the synthetic pending operation is staged");

        assert_eq!(
            workflow.create_event(&new_creation).await,
            Err(CalendarWorkflowError::Cache(
                CalendarCacheError::CreationAlreadyPending
            ))
        );
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );
    });
}

#[test]
fn definitive_event_creation_rejection_clears_its_pending_operation() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.fail_next_creation(CalendarOperationError::EventCreationRejected);
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "create-rejected-initial-sync").await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Ok(CalendarEventCreationOutcome::Rejected)
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the rejected event leaves no pending marker")
                .pending_creation()
                .is_none()
        );
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id()]
        );
    });
}

#[test]
fn rejected_event_cleanup_can_resume_without_reposting_to_graph() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.fail_next_creation(CalendarOperationError::EventCreationRejected);
        let cache = FixtureCache::default();
        cache.fail_rejection_cleanup(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "create-cleanup-initial-sync").await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Ok(CalendarEventCreationOutcome::RejectedButPending {
                cache_error: CalendarCacheError::Unavailable,
            })
        );
        let pending = workflow
            .load_cached_view(&view)
            .await
            .expect("the definitive rejection remains visible for local cleanup");
        assert_eq!(
            pending.pending_creation(),
            Some(&PendingCalendarEventCreation::Rejected(creation.clone()))
        );

        workflow
            .discard_rejected_event_creation(&creation)
            .await
            .expect("local cleanup retries without another provider request");
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the rejected marker is removed")
                .pending_creation()
                .is_none()
        );
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id()]
        );
    });
}

#[test]
fn rejected_event_can_be_resolved_after_persisting_the_rejection_recovers() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.fail_next_creation(CalendarOperationError::EventCreationRejected);
        let cache = FixtureCache::default();
        cache.fail_rejection_mark(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "create-rejection-mark-initial-sync",
        )
        .await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Ok(CalendarEventCreationOutcome::RejectedButPending {
                cache_error: CalendarCacheError::Unavailable,
            })
        );
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the unresolved draft remains available")
                .pending_creation(),
            Some(&PendingCalendarEventCreation::Unresolved(creation.clone()))
        );

        workflow
            .discard_rejected_event_creation(&creation)
            .await
            .expect("the stored rejection can be recorded and cleared locally");
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the rejected operation is cleared")
                .pending_creation()
                .is_none()
        );
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id()]
        );
    });
}

#[test]
fn event_creation_keeps_the_pending_operation_when_graph_confirms_another_event() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.return_creation(mismatched_creation_event(&creation));
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "create-mismatch-initial-sync").await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Err(CalendarWorkflowError::InvalidConfirmation)
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("unconfirmed operation remains visible for recovery");
        assert_eq!(
            snapshot.pending_creation().map(|item| item.creation().id()),
            Some(creation.id())
        );
        assert!(snapshot.events().is_empty());
    });
}

#[test]
fn event_update_confirms_and_replaces_the_cached_occurrence() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "event-update-initial-sync").await;
        let creation = creation(view.clone());
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation)
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic timed event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");

        let outcome = workflow
            .update_event(&update)
            .await
            .expect("synthetic Graph update is confirmed");
        assert!(matches!(outcome, CalendarEventUpdateOutcome::Cached(_)));
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("updated event remains cached");
        assert_eq!(snapshot.events().len(), 1);
        assert_eq!(snapshot.events()[0].subject(), Some("Updated planning"));
        assert_eq!(snapshot.events()[0].time(), update.draft().time());
    });
}

#[test]
fn event_update_reports_remote_confirmation_when_cache_commit_fails() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        cache.fail_update_commit(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "event-update-cache-initial-sync",
        )
        .await;
        let creation = creation(view.clone());
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation)
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic timed event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");

        let outcome = workflow
            .update_event(&update)
            .await
            .expect("a confirmed remote edit remains a successful outcome");
        assert!(matches!(
            outcome,
            CalendarEventUpdateOutcome::ConfirmedButNotCached {
                cache_error: CalendarCacheError::Unavailable,
                ..
            }
        ));
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the failed cache commit leaves the view readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.editable_event(&event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .len(),
            1,
            "a local cache failure must not repeat the remote update"
        );
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("after-update-cache-failure")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a completed sync clears the update recovery gate");
        assert!(workflow.editable_event(&event).await.is_ok());
    });
}

#[test]
fn confirmed_event_deletion_removes_the_cached_event() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "delete-initial-sync").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");

        assert_eq!(
            workflow
                .delete_event(&editable)
                .await
                .expect("provider deletion is confirmed"),
            CalendarEventDeletionOutcome::Deleted
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("deleted view remains readable");
        assert!(snapshot.events().is_empty());
        assert_eq!(snapshot.write_readiness(), CalendarWriteReadiness::Ready);
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1
        );
    });
}

#[test]
fn uncertain_event_deletion_requires_sync_and_is_not_replayed() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "delete-unknown-initial-sync").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        provider.fail_next_deletion(CalendarOperationError::DeleteOutcomeUnknown);

        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
            ))
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view remains readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "an uncertain deletion must not be replayed before sync"
        );

        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed {
                id: event.id().clone(),
            }],
            CalendarDeltaProgress::Complete(link("after-unknown-delete")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a full sync reconciles the deletion outcome");
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("reconciled view remains readable");
        assert!(snapshot.events().is_empty());
        assert_eq!(snapshot.write_readiness(), CalendarWriteReadiness::Ready);
    });
}

#[test]
fn uncertain_event_deletion_requires_sync_for_each_previously_synced_account_view() {
    smol::block_on(async {
        let source_view = view();
        let overlapping_view = view_for_week(14);
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        for (view, cursor) in [
            (&source_view, "delete-cross-week-source-initial"),
            (&overlapping_view, "delete-cross-week-overlap-initial"),
        ] {
            provider.push_page(Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: cross_week_timed_event_patch(),
                }],
                CalendarDeltaProgress::Complete(link(cursor)),
            )));
            workflow
                .sync_view(view)
                .await
                .expect("each synthetic view completes its initial sync");
        }

        let source_event = workflow
            .load_cached_view(&source_view)
            .await
            .expect("source event view is readable")
            .events()[0]
            .clone();
        let overlapping_event = workflow
            .load_cached_view(&overlapping_view)
            .await
            .expect("overlapping event view is readable")
            .events()[0]
            .clone();
        let source_editable = workflow
            .editable_event(&source_event)
            .await
            .expect("synchronized event can be opened for deletion");
        let overlapping_editable = workflow
            .editable_event(&overlapping_event)
            .await
            .expect("overlapping event can be opened for deletion");
        provider.fail_next_deletion(CalendarOperationError::DeleteOutcomeUnknown);

        assert_eq!(
            workflow.delete_event(&source_editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
            ))
        );
        for view in [&source_view, &overlapping_view] {
            assert!(
                workflow
                    .load_cached_view(view)
                    .await
                    .expect("each cached event view is readable")
                    .write_readiness()
                    == CalendarWriteReadiness::AccountSyncRequired
            );
        }
        assert_eq!(
            workflow.delete_event(&overlapping_editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed { id: event_id() }],
            CalendarDeltaProgress::Complete(link("delete-cross-week-source-recovered")),
        )));
        workflow
            .sync_view(&source_view)
            .await
            .expect("source view completes its recovery sync");
        assert!(
            workflow
                .load_cached_view(&source_view)
                .await
                .expect("reconciled source view is readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired,
            "the source view remains gated while another account view needs recovery"
        );
        assert_eq!(
            workflow.delete_event(&source_editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired),
            "one view's recovery sync must not unlock writes while another account view remains stale"
        );
        assert!(
            workflow
                .load_cached_view(&overlapping_view)
                .await
                .expect("overlapping view remains readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.delete_event(&overlapping_editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed { id: event_id() }],
            CalendarDeltaProgress::Complete(link("delete-cross-week-overlap-recovered")),
        )));
        workflow
            .sync_view(&overlapping_view)
            .await
            .expect("overlapping view completes its recovery sync");
        for view in [&source_view, &overlapping_view] {
            assert_eq!(
                workflow
                    .load_cached_view(view)
                    .await
                    .expect("reconciled event view is readable")
                    .write_readiness(),
                CalendarWriteReadiness::Ready
            );
        }
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "an uncertain deletion is not replayed before all views sync"
        );
    });
}

#[test]
fn changed_event_deletion_requires_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "delete-changed-initial-sync").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        provider.fail_next_deletion(CalendarOperationError::EventChanged);

        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::EventChanged
            ))
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached event remains available")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "a changed event is not deleted again before sync"
        );
    });
}

#[test]
fn confirmed_event_deletion_with_a_cache_failure_requires_sync() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        cache.fail_deletion_commit(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "delete-cache-initial-sync").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");

        assert_eq!(
            workflow.delete_event(&editable).await,
            Ok(CalendarEventDeletionOutcome::DeletedButNotCached {
                cache_error: CalendarCacheError::Unavailable,
            })
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view remains readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "a confirmed remote deletion is not repeated after a local failure"
        );
        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed {
                id: event.id().clone(),
            }],
            CalendarDeltaProgress::Complete(link("after-delete-cache-failure")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a full sync repairs the cache and clears the gate");
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("repaired view remains readable");
        assert!(snapshot.events().is_empty());
        assert_eq!(snapshot.write_readiness(), CalendarWriteReadiness::Ready);
    });
}

#[test]
fn event_deletion_reserves_the_account_until_the_provider_returns() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = std::sync::Arc::new(CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        ));
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "delete-reservation-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        let second_editable = editable.clone();
        let (started, release) = provider.hold_next_deletion();
        let deleting_workflow = std::sync::Arc::clone(&workflow);
        let deletion = smol::spawn(async move { deleting_workflow.delete_event(&editable).await });
        started
            .await
            .expect("synthetic deletion request has started");

        assert_eq!(
            workflow.delete_event(&second_editable).await,
            Err(CalendarWorkflowError::OperationInProgress)
        );
        release
            .send(())
            .expect("synthetic provider is waiting for deletion release");
        assert_eq!(deletion.await, Ok(CalendarEventDeletionOutcome::Deleted));
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1
        );
    });
}

#[test]
fn canceled_event_deletion_requires_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflows = workflows_sharing_account_state(&provider, &cache);
        synchronize_empty_view(
            &workflows.first,
            &provider,
            &view,
            "cancel-delete-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflows
            .first
            .create_event(&creation(view))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflows
            .first
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        let retry = editable.clone();
        let (started, _release) = provider.hold_next_deletion();
        let deleting_workflow = std::sync::Arc::clone(&workflows.first);
        let deletion = smol::spawn(async move { deleting_workflow.delete_event(&editable).await });
        started
            .await
            .expect("synthetic deletion request has started");

        assert!(deletion.cancel().await.is_none());
        assert_eq!(
            workflows.sibling.delete_event(&retry).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
    });
}

#[test]
fn reopened_workflow_requires_sync_for_every_previously_synced_view() {
    smol::block_on(async {
        let view = view();
        let other_view = view_for_week(14);
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(&workflow, &provider, &view, "restart-delete-source").await;
        synchronize_empty_view(&workflow, &provider, &other_view, "restart-delete-other").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        provider.fail_next_deletion(CalendarOperationError::DeleteOutcomeUnknown);

        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
            ))
        );

        let restarted = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("restart-delete-source-reconciled")),
        )));
        restarted
            .sync_view(&view)
            .await
            .expect("the source view completes its post-restart sync");
        assert_eq!(
            restarted
                .load_cached_view(&view)
                .await
                .expect("the source view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            restarted.create_event(&creation(view.clone())).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("restart-delete-other-reconciled")),
        )));
        restarted
            .sync_view(&other_view)
            .await
            .expect("the other previously synced view completes its post-restart sync");
        assert_eq!(
            restarted
                .load_cached_view(&view)
                .await
                .expect("all account views are reconciled")
                .write_readiness(),
            CalendarWriteReadiness::Ready
        );
        assert!(matches!(
            restarted.create_event(&creation(view)).await,
            Ok(CalendarEventCreationOutcome::Cached(_))
        ));
    });
}

#[test]
fn unknown_event_update_requires_successful_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "unknown-event-update-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic timed event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");
        let followup_creation = creation(view.clone());
        provider.fail_next_update(CalendarOperationError::UpdateOutcomeUnknown);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UpdateOutcomeUnknown
            ))
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached event stays available")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.editable_event(&event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.create_event(&followup_creation).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the unresolved update leaves the view readable")
                .pending_creation()
                .is_none()
        );
        assert_eq!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .len(),
            1,
            "a recovery-required account must not send another event creation"
        );
        assert!(matches!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        ));
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("after-unknown-update")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a complete sync clears the recovery gate");
        assert!(workflow.editable_event(&event).await.is_ok());
        assert!(matches!(
            workflow.create_event(&followup_creation).await,
            Ok(CalendarEventCreationOutcome::Cached(_))
        ));
        assert_eq!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .len(),
            1,
            "uncertain updates are not replayed after sync"
        );
    });
}

#[test]
fn uncertain_update_and_delete_recovery_blocks_sibling_workflows() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflows = workflows_sharing_account_state(&provider, &cache);
        synchronize_empty_view(
            &workflows.first,
            &provider,
            &view,
            "shared-recovery-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflows
            .first
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflows
            .first
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        let update = update_request(editable.clone());
        let sibling_update = update_request(editable.clone());
        provider.fail_next_update(CalendarOperationError::UpdateOutcomeUnknown);

        assert_eq!(
            workflows.first.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UpdateOutcomeUnknown
            ))
        );
        assert_eq!(
            workflows.sibling.update_event(&sibling_update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        synchronize_empty_view(
            &workflows.sibling,
            &provider,
            &view,
            "shared-recovery-update-sync",
        )
        .await;
        let editable = workflows
            .sibling
            .editable_event(&event)
            .await
            .expect("successful sync restores calendar writes");
        let sibling_editable = editable.clone();
        provider.fail_next_deletion(CalendarOperationError::DeleteOutcomeUnknown);

        assert_eq!(
            workflows.first.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
            ))
        );
        assert_eq!(
            workflows.sibling.delete_event(&sibling_editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
    });
}

#[test]
fn definitive_event_write_rejections_clear_persisted_recovery_markers() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "definitive-write-rejection-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        let update = update_request(editable);
        provider.fail_next_update(CalendarOperationError::AccessDenied);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::AccessDenied
            ))
        );
        assert!(
            !cache
                .account_has_calendar_write_sync_required(view.account_id())
                .await
                .expect("definitive update rejection clears its recovery marker")
        );
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::Ready
        );

        let editable = workflow
            .editable_event(&event)
            .await
            .expect("definitive rejection keeps the event editable");
        provider.fail_next_deletion(CalendarOperationError::EventDeletionRejected);
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::EventDeletionRejected
            ))
        );
        assert!(
            !cache
                .account_has_calendar_write_sync_required(view.account_id())
                .await
                .expect("definitive deletion rejection clears its recovery marker")
        );
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view stays ready after definitive rejection")
                .write_readiness(),
            CalendarWriteReadiness::Ready
        );
    });
}

#[test]
fn failed_recovery_marker_cleanup_keeps_calendar_writes_blocked() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(&workflow, &provider, &view, "cleanup-failure-initial-sync").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        cache.fail_write_recovery_cleanup(CalendarCacheError::Unavailable);
        provider.fail_next_deletion(CalendarOperationError::EventDeletionRejected);

        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Cache(
                CalendarCacheError::Unavailable
            ))
        );
        assert!(
            cache
                .account_has_calendar_write_sync_required(view.account_id())
                .await
                .expect("failed cleanup leaves the persistent recovery marker")
        );
        assert_eq!(
            workflow.create_event(&creation(view.clone())).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "the rejected deletion is attempted only once"
        );
        assert_eq!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .len(),
            1,
            "a new event write does not reach the provider"
        );
    });
}

#[test]
fn changed_event_update_requires_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "update-changed-initial-sync").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");
        provider.fail_next_update(CalendarOperationError::EventChanged);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::EventChanged
            ))
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached event remains available")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .len(),
            1,
            "a changed event is not updated again before sync"
        );
    });
}

#[test]
fn uncertain_update_requires_sync_for_each_previously_synced_account_view() {
    smol::block_on(async {
        let source_view = view();
        let overlapping_view = view_for_week(14);
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        for (view, cursor) in [
            (&source_view, "cross-week-source-initial"),
            (&overlapping_view, "cross-week-overlap-initial"),
        ] {
            provider.push_page(Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: cross_week_timed_event_patch(),
                }],
                CalendarDeltaProgress::Complete(link(cursor)),
            )));
            workflow
                .sync_view(view)
                .await
                .expect("each synthetic view completes its initial sync");
        }

        let source_event = workflow
            .load_cached_view(&source_view)
            .await
            .expect("source event view is readable")
            .events()[0]
            .clone();
        let overlapping_event = workflow
            .load_cached_view(&overlapping_view)
            .await
            .expect("overlapping event view is readable")
            .events()[0]
            .clone();
        let editable = workflow
            .editable_event(&source_event)
            .await
            .expect("synchronized event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated cross-week appointment".to_owned(),
            utc(2030, Month::January, 13) + time::Duration::hours(9),
            utc(2030, Month::January, 15) + time::Duration::hours(10),
        )
        .expect("cross-week edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("the subject changed");
        provider.fail_next_update(CalendarOperationError::UpdateOutcomeUnknown);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UpdateOutcomeUnknown
            ))
        );
        for view in [&source_view, &overlapping_view] {
            assert!(
                workflow
                    .load_cached_view(view)
                    .await
                    .expect("each cached event view is readable")
                    .write_readiness()
                    == CalendarWriteReadiness::AccountSyncRequired
            );
        }
        assert_eq!(
            workflow.editable_event(&overlapping_event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        for (view, cursor) in [
            (&source_view, "cross-week-source-recovered"),
            (&overlapping_view, "cross-week-overlap-recovered"),
        ] {
            let mut patch = CalendarOccurrencePatch::new();
            patch.set_subject(Some("Updated cross-week appointment".to_owned()));
            provider.push_page(Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch,
                }],
                CalendarDeltaProgress::Complete(link(cursor)),
            )));
            workflow
                .sync_view(view)
                .await
                .expect("each view independently reconciles the appointment");
            assert_eq!(
                workflow
                    .load_cached_view(view)
                    .await
                    .expect("reconciled view is readable")
                    .write_readiness(),
                if view == &source_view {
                    CalendarWriteReadiness::AccountSyncRequired
                } else {
                    CalendarWriteReadiness::Ready
                },
                "every account view stays gated until the last recovery sync completes"
            );
            if view == &source_view {
                assert!(
                    workflow
                        .load_cached_view(&overlapping_view)
                        .await
                        .expect("the other view remains readable")
                        .write_readiness()
                        == CalendarWriteReadiness::AccountSyncRequired
                );
            }
        }
        let reconciled = workflow
            .load_cached_view(&overlapping_view)
            .await
            .expect("overlapping event view is reconciled")
            .events()[0]
            .clone();
        assert_eq!(reconciled.subject(), Some("Updated cross-week appointment"));
        assert!(workflow.editable_event(&reconciled).await.is_ok());
    });
}

#[test]
fn event_update_reserves_the_whole_account_until_the_cache_commit_finishes() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow = std::sync::Arc::new(CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            cache,
        ));
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "event-update-reservation-initial-sync",
        )
        .await;
        let creation = creation(view.clone());
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation)
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("synthetic timed event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");
        let (started, release) = provider.hold_next_update();
        let updating_workflow = std::sync::Arc::clone(&workflow);
        let update_task = smol::spawn(async move { updating_workflow.update_event(&update).await });
        started
            .await
            .expect("synthetic provider started the held update");

        let another_week = view_for_week(14);
        assert_eq!(
            workflow.sync_view(&another_week).await,
            Err(CalendarWorkflowError::OperationInProgress)
        );
        release
            .send(())
            .expect("the synthetic update request is waiting for release");
        assert!(matches!(
            update_task.await,
            Ok(CalendarEventUpdateOutcome::Cached(_))
        ));
    });
}

#[test]
fn canceled_event_update_requires_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflows = workflows_sharing_account_state(&provider, &cache);
        synchronize_empty_view(
            &workflows.first,
            &provider,
            &view,
            "cancel-update-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflows
            .first
            .create_event(&creation(view))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflows
            .first
            .editable_event(&event)
            .await
            .expect("synthetic event is editable");
        let update = update_request(editable.clone());
        let retry = update_request(editable);
        let (started, _release) = provider.hold_next_update();
        let updating_workflow = std::sync::Arc::clone(&workflows.first);
        let update_task = smol::spawn(async move { updating_workflow.update_event(&update).await });
        started.await.expect("synthetic update request has started");

        assert!(update_task.cancel().await.is_none());
        assert_eq!(
            workflows.sibling.update_event(&retry).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
    });
}

#[test]
fn event_creation_reserves_the_account_while_another_view_sync_is_in_flight() {
    smol::block_on(async {
        let view = view();
        let another_view = view_for_week(6);
        let provider = FixtureProvider::default();
        let workflow = std::sync::Arc::new(CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        ));
        synchronize_empty_view(&workflow, &provider, &view, "create-sync-race-source").await;
        synchronize_empty_view(
            &workflow,
            &provider,
            &another_view,
            "create-sync-race-target",
        )
        .await;
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("create-sync-race-followup")),
        )));

        let (started, release) = provider.hold_next_delta();
        let syncing_workflow = std::sync::Arc::clone(&workflow);
        let syncing_view = view.clone();
        let sync = smol::spawn(async move { syncing_workflow.sync_view(&syncing_view).await });
        started
            .await
            .expect("the synthetic delta request has started");

        assert_eq!(
            workflow.create_event(&creation(another_view)).await,
            Err(CalendarWorkflowError::OperationInProgress)
        );
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );

        release
            .send(())
            .expect("the synthetic delta request is waiting for release");
        assert!(sync.await.is_ok());
    });
}

fn view_for_week(start_day: u8) -> CalendarViewReference {
    let account = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account identifier is valid");
    let start_date = Date::from_calendar_date(2030, Month::January, start_day)
        .expect("synthetic view start date is valid");
    let end_date = start_date
        .checked_add(time::Duration::days(7))
        .expect("synthetic view end date is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic calendar view is valid");
    CalendarViewReference::new(account, range)
}

#[test]
fn replacement_sync_keeps_the_old_snapshot_visible_until_final_commit() {
    smol::block_on(async {
        let view = view();
        let old_delta = link("synthetic-old-delta");
        let reset_link = link("synthetic-reset");
        let replacement_next = link("synthetic-replacement-next");
        let new_delta = link("synthetic-new-delta");
        let provider = FixtureProvider::with_pages([Ok(response(
            vec![CalendarDeltaChange::Upsert {
                id: event_id(),
                patch: all_day_patch(Some("Old snapshot"), None),
            }],
            CalendarDeltaProgress::Complete(old_delta.clone()),
        ))]);
        let request_log = Arc::clone(&provider.requests);
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        workflow
            .sync_view(&view)
            .await
            .expect("the initial snapshot commits");
        provider.push_page(Ok(CalendarDeltaResponse::Reset {
            reset_link: Some(reset_link.clone()),
            response_bytes: 96,
        }));
        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Upsert {
                id: replacement_event_id(),
                patch: all_day_patch(Some("Replacement snapshot"), None),
            }],
            CalendarDeltaProgress::Next(replacement_next.clone()),
        )));
        provider.push_page(Err(CalendarOperationError::ServiceUnavailable));

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        let interrupted = workflow
            .load_cached_view(&view)
            .await
            .expect("the previous complete snapshot remains readable");
        assert_eq!(interrupted.events().len(), 1);
        assert_eq!(interrupted.events()[0].subject(), Some("Old snapshot"));
        assert_eq!(interrupted.sync_state().delta_link(), Some(&old_delta));
        assert_eq!(
            interrupted.sync_state().next_link(),
            Some(&replacement_next)
        );
        assert!(interrupted.sync_state().replacement_active());

        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(new_delta.clone()),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("the replacement snapshot completes");
        let completed = workflow
            .load_cached_view(&view)
            .await
            .expect("the replacement snapshot is readable");
        assert_eq!(completed.events().len(), 1);
        assert_eq!(
            completed.events()[0].subject(),
            Some("Replacement snapshot")
        );
        assert_eq!(completed.sync_state().delta_link(), Some(&new_delta));
        assert!(!completed.sync_state().replacement_active());
        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(old_delta),
                CalendarDeltaRequest::Continue(reset_link),
                CalendarDeltaRequest::Continue(replacement_next.clone()),
                CalendarDeltaRequest::Continue(replacement_next),
            ]
        );
    });
}

#[test]
fn a_second_reset_is_persisted_and_stops_the_current_attempt() {
    smol::block_on(async {
        let view = view();
        let old_delta = link("synthetic-old-delta");
        let reset_link = link("synthetic-reset");
        let provider = FixtureProvider::with_pages([Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(old_delta.clone()),
        ))]);
        let request_log = Arc::clone(&provider.requests);
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        workflow
            .sync_view(&view)
            .await
            .expect("the initial cursor commits");

        provider.push_page(Ok(CalendarDeltaResponse::Reset {
            reset_link: Some(reset_link.clone()),
            response_bytes: 24,
        }));
        provider.push_page(Ok(CalendarDeltaResponse::Reset {
            reset_link: None,
            response_bytes: 32,
        }));
        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::DeltaResetLimitExceeded)
        );
        let checkpoint = workflow
            .load_cached_view(&view)
            .await
            .expect("the replacement checkpoint remains readable");
        assert_eq!(checkpoint.sync_state().delta_link(), Some(&old_delta));
        assert!(checkpoint.sync_state().replacement_active());
        assert_eq!(
            checkpoint.sync_state().request(),
            CalendarDeltaRequest::Initial
        );

        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("synthetic-recovered")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a later attempt resumes the fixed-range initial request");
        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(old_delta),
                CalendarDeltaRequest::Continue(reset_link),
                CalendarDeltaRequest::Initial,
            ]
        );
    });
}

#[test]
fn event_writes_wait_for_an_interrupted_calendar_replacement_sync() {
    smol::block_on(async {
        let view = view();
        let another_view = view_for_week(8);
        let old_delta = link("synthetic-old-delta");
        let reset_link = link("synthetic-reset");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: cross_week_timed_event_patch(),
                }],
                CalendarDeltaProgress::Complete(old_delta.clone()),
            )),
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: cross_week_timed_event_patch(),
                }],
                CalendarDeltaProgress::Complete(link("synthetic-another-view")),
            )),
        ]);
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        workflow
            .sync_view(&view)
            .await
            .expect("the initial event snapshot commits");
        workflow
            .sync_view(&another_view)
            .await
            .expect("the overlapping event view commits");
        let event = workflow
            .load_cached_view(&view)
            .await
            .expect("the initial event is readable")
            .events()[0]
            .clone();
        let another_event = workflow
            .load_cached_view(&another_view)
            .await
            .expect("the overlapping event is readable")
            .events()[0]
            .clone();
        let editable = workflow
            .editable_event(&event)
            .await
            .expect("the synchronized event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("the synthetic update draft is valid");
        let update = CalendarEventUpdate::new(editable.clone(), draft)
            .expect("the update differs from the source event");

        provider.push_page(Ok(CalendarDeltaResponse::Reset {
            reset_link: Some(reset_link),
            response_bytes: 64,
        }));
        provider.push_page(Err(CalendarOperationError::ServiceUnavailable));
        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );

        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the interrupted replacement remains readable")
                .write_readiness(),
            CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow
                .load_cached_view(&another_view)
                .await
                .expect("the other account view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.create_event(&creation(another_view.clone())).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.editable_event(&another_event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );
        assert!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .is_empty()
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion log mutex is not poisoned"),
            0
        );
    });
}
