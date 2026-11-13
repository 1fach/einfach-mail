struct CalendarWriteGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

#[derive(Clone, Default)]
pub(super) struct FixtureProvider {
    calendar_capabilities: Option<CalendarCapabilities>,
    pages: Arc<Mutex<VecDeque<Result<CalendarDeltaResponse, CalendarOperationError>>>>,
    pub(super) sync_strategy: Arc<Mutex<Option<CalendarSyncStrategy>>>,
    pub(super) snapshots:
        Arc<Mutex<VecDeque<Result<CalendarViewSnapshotResponse, CalendarOperationError>>>>,
    delta_gate: Arc<Mutex<Option<CalendarWriteGate>>>,
    snapshot_gate: Arc<Mutex<Option<CalendarWriteGate>>>,
    pub(super) requests: Arc<Mutex<Vec<CalendarDeltaRequest>>>,
    pub(super) creations: Arc<Mutex<Vec<CalendarEventCreationId>>>,
    pub(super) recoveries: Arc<Mutex<Vec<CalendarEventCreationId>>>,
    creation_error: Arc<Mutex<Option<CalendarOperationError>>>,
    recovery_error: Arc<Mutex<Option<CalendarOperationError>>>,
    creation_override: Arc<Mutex<Option<CalendarOccurrence>>>,
    pub(super) update_calls: Arc<Mutex<Vec<CalendarEventUpdate>>>,
    update_gate: Arc<Mutex<Option<CalendarWriteGate>>>,
    update_error: Arc<Mutex<Option<CalendarOperationError>>>,
    pub(super) deletion_calls: Arc<Mutex<usize>>,
    pub(super) deletion_events: Arc<Mutex<Vec<VersionedCalendarEvent>>>,
    deletion_gate: Arc<Mutex<Option<CalendarWriteGate>>>,
    deletion_error: Arc<Mutex<Option<CalendarOperationError>>>,
}

impl FixtureProvider {
    pub(super) fn with_calendar_capabilities(
        mut self,
        capabilities: CalendarCapabilities,
    ) -> Self {
        self.calendar_capabilities = Some(capabilities);
        self
    }

    pub(super) fn with_pages(
        pages: impl IntoIterator<Item = Result<CalendarDeltaResponse, CalendarOperationError>>,
    ) -> Self {
        Self {
            calendar_capabilities: None,
            pages: Arc::new(Mutex::new(pages.into_iter().collect())),
            sync_strategy: Arc::new(Mutex::new(None)),
            snapshots: Arc::new(Mutex::new(VecDeque::new())),
            delta_gate: Arc::new(Mutex::new(None)),
            snapshot_gate: Arc::new(Mutex::new(None)),
            requests: Arc::new(Mutex::new(Vec::new())),
            creations: Arc::new(Mutex::new(Vec::new())),
            recoveries: Arc::new(Mutex::new(Vec::new())),
            creation_error: Arc::new(Mutex::new(None)),
            recovery_error: Arc::new(Mutex::new(None)),
            creation_override: Arc::new(Mutex::new(None)),
            update_calls: Arc::new(Mutex::new(Vec::new())),
            update_gate: Arc::new(Mutex::new(None)),
            update_error: Arc::new(Mutex::new(None)),
            deletion_calls: Arc::new(Mutex::new(0)),
            deletion_events: Arc::new(Mutex::new(Vec::new())),
            deletion_gate: Arc::new(Mutex::new(None)),
            deletion_error: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn with_snapshots(
        snapshots: impl IntoIterator<
            Item = Result<CalendarViewSnapshotResponse, CalendarOperationError>,
        >,
    ) -> Self {
        let provider = Self::default();
        *provider
            .sync_strategy
            .lock()
            .expect("synthetic strategy mutex is not poisoned") =
            Some(CalendarSyncStrategy::Snapshot);
        provider
            .snapshots
            .lock()
            .expect("synthetic snapshot queue mutex is not poisoned")
            .extend(snapshots);
        provider
    }

    pub(super) fn push_page(&self, page: Result<CalendarDeltaResponse, CalendarOperationError>) {
        self.pages
            .lock()
            .expect("synthetic page queue mutex is not poisoned")
            .push_back(page);
    }

    pub(super) fn hold_next_delta(
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

    pub(super) fn hold_next_snapshot(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self
            .snapshot_gate
            .lock()
            .expect("synthetic snapshot gate mutex is not poisoned") = Some(CalendarWriteGate {
            started: started_tx,
            release: release_rx,
        });
        (started_rx, release_tx)
    }

    pub(super) fn fail_next_creation(&self, error: CalendarOperationError) {
        *self
            .creation_error
            .lock()
            .expect("synthetic creation error mutex is not poisoned") = Some(error);
    }

    pub(super) fn fail_next_recovery(&self, error: CalendarOperationError) {
        *self
            .recovery_error
            .lock()
            .expect("synthetic recovery error mutex is not poisoned") = Some(error);
    }

    pub(super) fn return_creation(&self, event: CalendarOccurrence) {
        *self
            .creation_override
            .lock()
            .expect("synthetic creation result mutex is not poisoned") = Some(event);
    }

    pub(super) fn fail_next_update(&self, error: CalendarOperationError) {
        *self
            .update_error
            .lock()
            .expect("synthetic update error mutex is not poisoned") = Some(error);
    }

    pub(super) fn hold_next_update(
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

    pub(super) fn fail_next_deletion(&self, error: CalendarOperationError) {
        *self
            .deletion_error
            .lock()
            .expect("synthetic deletion error mutex is not poisoned") = Some(error);
    }

    pub(super) fn hold_next_deletion(
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
    fn calendar_capabilities(
        &self,
    ) -> impl Future<Output = Result<CalendarCapabilities, CalendarOperationError>> + Send + '_ {
        ready(Ok(self
            .calendar_capabilities
            .unwrap_or_else(CalendarCapabilities::full_access)))
    }

    fn calendar_sync_strategy(
        &self,
    ) -> impl Future<Output = Result<CalendarSyncStrategy, CalendarOperationError>> + Send + '_
    {
        let strategy = *self
            .sync_strategy
            .lock()
            .expect("synthetic strategy mutex is not poisoned");
        ready(Ok(strategy.unwrap_or(CalendarSyncStrategy::Delta)))
    }

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

    fn calendar_view_snapshot(
        &self,
        _: &CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarViewSnapshotResponse, CalendarOperationError>> + Send + '_
    {
        let gate = self
            .snapshot_gate
            .lock()
            .expect("synthetic snapshot gate mutex is not poisoned")
            .take();
        let snapshots = Arc::clone(&self.snapshots);
        async move {
            if let Some(gate) = gate {
                gate.started
                    .send(())
                    .expect("the calendar snapshot test is waiting at its provider gate");
                gate.release
                    .await
                    .expect("the calendar snapshot test releases its provider gate");
            }
            snapshots
                .lock()
                .expect("synthetic snapshot queue mutex is not poisoned")
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
            synthetic_creation_result(&creation, &creation_error, &creation_override)
        }
    }

    fn recover_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let creation = creation.clone();
        let recoveries = Arc::clone(&self.recoveries);
        let recovery_error = Arc::clone(&self.recovery_error);
        let creation_override = Arc::clone(&self.creation_override);
        async move {
            recoveries
                .lock()
                .expect("synthetic recovery log mutex is not poisoned")
                .push(creation.id());
            synthetic_creation_result(&creation, &recovery_error, &creation_override)
        }
    }

    fn load_versioned_event(
        &self,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<VersionedCalendarEvent, CalendarOperationError>> + Send + '_
    {
        let event = event.clone();
        async move {
            VersionedCalendarEvent::new(
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
        event: &VersionedCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        let deletion_calls = Arc::clone(&self.deletion_calls);
        let deletion_events = Arc::clone(&self.deletion_events);
        let event = event.clone();
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
            deletion_events
                .lock()
                .expect("synthetic deletion event mutex is not poisoned")
                .push(event);
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

fn synthetic_creation_result(
    creation: &CalendarEventCreation,
    error: &Mutex<Option<CalendarOperationError>>,
    override_event: &Mutex<Option<CalendarOccurrence>>,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    if let Some(error) = error
        .lock()
        .expect("synthetic provider failure mutex is not poisoned")
        .take()
    {
        return Err(error);
    }
    if let Some(event) = override_event
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
