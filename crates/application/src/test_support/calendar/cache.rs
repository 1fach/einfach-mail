#[derive(Clone, Default)]
pub(super) struct FixtureCache {
    views: Arc<Mutex<HashMap<CalendarViewReference, CachedView>>>,
    pending: Arc<Mutex<HashMap<AccountId, PendingCalendarEventCreation>>>,
    creation_commit_error: Arc<Mutex<Option<CalendarCacheError>>>,
    rejection_mark_error: Arc<Mutex<Option<CalendarCacheError>>>,
    rejection_cleanup_error: Arc<Mutex<Option<CalendarCacheError>>>,
    update_commit_error: Arc<Mutex<Option<CalendarCacheError>>>,
    deletion_commit_error: Arc<Mutex<Option<CalendarCacheError>>>,
    snapshot_commit_error: Arc<Mutex<Option<CalendarCacheError>>>,
    write_recovery_cleanup_error: Arc<Mutex<Option<CalendarCacheError>>>,
}

impl FixtureCache {
    pub(super) fn fail_creation_commit(&self, error: CalendarCacheError) {
        *self
            .creation_commit_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    pub(super) fn fail_rejection_cleanup(&self, error: CalendarCacheError) {
        *self
            .rejection_cleanup_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    pub(super) fn fail_rejection_mark(&self, error: CalendarCacheError) {
        *self
            .rejection_mark_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    pub(super) fn fail_update_commit(&self, error: CalendarCacheError) {
        *self
            .update_commit_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    pub(super) fn fail_deletion_commit(&self, error: CalendarCacheError) {
        *self
            .deletion_commit_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    pub(super) fn fail_snapshot_commit(&self, error: CalendarCacheError) {
        *self
            .snapshot_commit_error
            .lock()
            .expect("synthetic cache error mutex is not poisoned") = Some(error);
    }

    pub(super) fn fail_write_recovery_cleanup(&self, error: CalendarCacheError) {
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
            if views
                .get(&view)
                .is_none_or(|cached| cached.state.last_synchronized_at().is_none())
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
        let snapshot_commit_error = Arc::clone(&self.snapshot_commit_error);
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

            if let Some(error) = snapshot_commit_error
                .lock()
                .expect("synthetic cache error mutex is not poisoned")
                .take()
            {
                return Err(error);
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
        event: &VersionedCalendarEvent,
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
