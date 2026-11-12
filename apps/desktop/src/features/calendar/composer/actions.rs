use super::state::*;
use super::*;

impl<S: CalendarSession + ?Sized + 'static> EventComposer<S> {
    pub(super) fn start_edit_load(
        &mut self,
        cached_event: CalendarOccurrence,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let versioned_event = session.load_versioned_event(&cached_event).await;
            let _ = this.update_in(cx, |composer, window, cx| {
                composer.apply_versioned_event(versioned_event, window, cx);
            });
        }));
    }

    fn apply_versioned_event(
        &mut self,
        versioned_result: Result<VersionedCalendarEvent, CalendarWorkflowError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        match versioned_result {
            Ok(versioned_event) => self.load_edit_fields(versioned_event, window, cx),
            Err(error) => self.state = ComposerState::Edit(EditState::LoadFailed(error)),
        }
        cx.notify();
    }

    fn load_edit_fields(
        &mut self,
        versioned_event: VersionedCalendarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let time_zone = self.week.time_zone();
        let Some(form_values) =
            super::time::editable_event_picker_values(&versioned_event, &time_zone)
        else {
            self.state = ComposerState::Edit(EditState::LoadFailed(
                CalendarWorkflowError::InvalidVersionedEvent,
            ));
            return;
        };
        self.set_edit_form_values(form_values, window, cx);
        self.state = ComposerState::Edit(EditState::Ready {
            versioned_event: Box::new(versioned_event),
            status: EditStatus::Editing,
        });
    }

    fn set_edit_form_values(
        &mut self,
        form_values: EditableEventFormValues,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.subject.update(cx, |input, cx| {
            input.set_value(form_values.subject, window, cx);
        });
        self.start.update(cx, |picker, cx| {
            picker.set_date_time(form_values.start, window, cx);
        });
        self.end.update(cx, |picker, cx| {
            picker.set_date_time(form_values.end, window, cx);
        });
    }

    pub(super) fn activate_primary_action(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit() {
            return;
        }
        match &self.state {
            ComposerState::Create(CreateState::Pending {
                status:
                    PendingCreationStatus::RejectedPendingCleanup
                    | PendingCreationStatus::RejectedCleanupFailed(_),
                ..
            }) => self.discard_rejected_creation(cx),
            ComposerState::Create(CreateState::Pending { .. }) => {
                self.check_pending_creation_status(cx);
            }
            ComposerState::Create(
                CreateState::Draft | CreateState::DraftFailed(_) | CreateState::InvalidDraft(_),
            ) => self.submit_create(cx),
            ComposerState::Create(
                CreateState::PendingResolutionRequired
                | CreateState::PendingResolutionFailed(_)
                | CreateState::Completed(_),
            ) => {}
            ComposerState::Edit(EditState::Ready { .. }) => self.submit_edit(cx),
            ComposerState::Edit(EditState::Loading | EditState::LoadFailed(_)) => {}
        }
    }

    fn submit_create(&mut self, cx: &mut Context<Self>) {
        let Some(creation) = self.creation_for_submission(cx) else {
            return;
        };
        match creation {
            Ok(creation) => self.submit_creation(creation, cx),
            Err(error) => {
                self.state = ComposerState::Create(CreateState::InvalidDraft(error));
                cx.notify();
            }
        }
    }

    fn submit_edit(&mut self, cx: &mut Context<Self>) {
        let update = match self.versioned_event() {
            Some(versioned_event) => self.update_for_submission(versioned_event, cx),
            None => return,
        };
        match update {
            Ok(update) => self.submit_update(update, cx),
            Err(error) => {
                self.set_edit_status(EditStatus::InvalidDraft(error));
                cx.notify();
            }
        }
    }

    fn submit_creation(&mut self, creation: CalendarEventCreation, cx: &mut Context<Self>) {
        self.state = ComposerState::Create(CreateState::Pending {
            creation: creation.clone(),
            status: PendingCreationStatus::Creating,
        });
        self.start_submission(creation, cx);
        cx.notify();
    }

    fn check_pending_creation_status(&mut self, cx: &mut Context<Self>) {
        let Some((creation, _)) = self.state.pending_creation() else {
            return;
        };
        let creation = creation.clone();
        if let ComposerState::Create(CreateState::Pending { status, .. }) = &mut self.state {
            *status = PendingCreationStatus::Checking;
        }
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let recovery_outcome = session.recover_event_creation(&creation).await;
            let _ = this.update(cx, |composer, cx| {
                composer.apply_recovery(recovery_outcome, cx);
            });
        }));
        cx.notify();
    }

    fn discard_rejected_creation(&mut self, cx: &mut Context<Self>) {
        let creation = match &self.state {
            ComposerState::Create(CreateState::Pending {
                creation,
                status:
                    PendingCreationStatus::RejectedPendingCleanup
                    | PendingCreationStatus::RejectedCleanupFailed(_),
            }) => creation.clone(),
            _ => return,
        };
        if let ComposerState::Create(CreateState::Pending { status, .. }) = &mut self.state {
            *status = PendingCreationStatus::DiscardingRejected;
        }
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = session.discard_rejected_event_creation(&creation).await;
            let _ = this.update(cx, |composer, cx| {
                composer.task = None;
                match result {
                    Ok(()) => {
                        if matches!(
                            &composer.state,
                            ComposerState::Create(CreateState::Pending { .. })
                        ) {
                            composer.state = ComposerState::Create(CreateState::Completed(
                                CreationOutcome::Rejected,
                            ));
                            cx.emit(EventComposerEvent::PendingOperationUpdated);
                        }
                    }
                    Err(error) => {
                        if let ComposerState::Create(CreateState::Pending { status, .. }) =
                            &mut composer.state
                        {
                            *status = PendingCreationStatus::RejectedCleanupFailed(error);
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn start_submission(&mut self, creation: CalendarEventCreation, cx: &mut Context<Self>) {
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let creation_outcome = session.create_event(&creation).await;
            let conflict_recovery =
                recover_pending_conflict(session.as_ref(), &creation, &creation_outcome).await;
            let _ = this.update(cx, |composer, cx| {
                composer.apply_submission(creation_outcome, conflict_recovery, cx);
            });
        }));
    }

    fn update_for_submission(
        &self,
        versioned_event: &VersionedCalendarEvent,
        cx: &App,
    ) -> Result<CalendarEventUpdate, DraftError> {
        let subject = self.event_subject(cx)?;
        let (selected_start, selected_end) = self.selected_local_times(cx)?;
        let CalendarOccurrenceTime::Timed {
            start: source_start,
            end: source_end,
        } = versioned_event.occurrence().time()
        else {
            return Err(DraftError::InvalidTime);
        };
        let time_zone = self.week.time_zone();
        let start = super::time::selected_event_time(selected_start, *source_start, &time_zone)?;
        let end = super::time::selected_event_time(selected_end, *source_end, &time_zone)?;
        let draft = CalendarEventUpdateDraft::timed(subject, start, end)
            .map_err(|_| DraftError::InvalidTime)?;
        CalendarEventUpdate::new(versioned_event.clone(), draft).map_err(|_| DraftError::NoChanges)
    }

    pub(crate) fn edit_write_in_progress(&self) -> bool {
        self.state.edit_write_in_progress()
    }

    fn versioned_event(&self) -> Option<&VersionedCalendarEvent> {
        match &self.state {
            ComposerState::Edit(EditState::Ready {
                versioned_event, ..
            }) => Some(versioned_event),
            ComposerState::Create(_)
            | ComposerState::Edit(EditState::Loading | EditState::LoadFailed(_)) => None,
        }
    }

    pub(super) fn confirm_deletion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_delete() {
            return;
        }
        let Some(event) = self.versioned_event() else {
            return;
        };
        let subject = event
            .occurrence()
            .subject()
            .unwrap_or("this appointment")
            .to_owned();
        let composer = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let composer = composer.clone();
            alert
                .title(format!("Delete “{subject}”?"))
                .description("This removes the appointment from your Microsoft calendar. This cannot be undone.")
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| {
                            button
                                .debug_selector(|| "event-delete-dialog-cancel".to_owned())
                                .label("Cancel")
                        }))
                        .child(
                            DialogAction::new().child(
                                Button::new("event-delete-dialog-confirm")
                                    .debug_selector(|| "event-delete-dialog-confirm".to_owned())
                                    .danger()
                                    .label("Delete"),
                            ),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let _ = composer.update(cx, |composer, cx| composer.start_deletion(cx));
                    true
                })
        });
    }

    fn start_deletion(&mut self, cx: &mut Context<Self>) {
        let Some(event) = self.versioned_event().cloned() else {
            return;
        };
        if !self.can_delete() {
            return;
        }
        if let ComposerState::Edit(EditState::Ready { status, .. }) = &mut self.state {
            *status = EditStatus::Deleting;
        }
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let outcome = session.delete_event(&event).await;
            let _ = this.update(cx, |composer, cx| composer.apply_deletion(outcome, cx));
        }));
        cx.notify();
    }

    fn apply_deletion(
        &mut self,
        outcome: Result<CalendarEventDeletionOutcome, CalendarWorkflowError>,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        let sync_required = matches!(
            &outcome,
            Ok(CalendarEventDeletionOutcome::DeletedButNotCached { .. })
                | Err(CalendarWorkflowError::Provider(
                    CalendarOperationError::DeleteOutcomeUnknown
                ))
                | Err(CalendarWorkflowError::Provider(
                    CalendarOperationError::EventChanged
                ))
                | Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        let status = match outcome {
            Ok(CalendarEventDeletionOutcome::Deleted) => EditStatus::Deleted,
            Ok(CalendarEventDeletionOutcome::DeletedButNotCached { .. }) => {
                EditStatus::DeletedButNotCached
            }
            Err(CalendarWorkflowError::Provider(CalendarOperationError::DeleteOutcomeUnknown)) => {
                EditStatus::DeleteOutcomeUnknown
            }
            Err(error) => EditStatus::DeletionFailed(error),
        };
        self.set_edit_status(status);
        if sync_required {
            cx.emit(EventComposerEvent::CalendarWriteSyncRequired);
        }
        cx.emit(EventComposerEvent::CalendarChanged);
        cx.notify();
    }

    fn submit_update(&mut self, update: CalendarEventUpdate, cx: &mut Context<Self>) {
        self.set_edit_status(EditStatus::Submitting);
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let outcome = session.update_event(&update).await;
            let _ = this.update(cx, |composer, cx| composer.apply_update(outcome, cx));
        }));
        cx.notify();
    }

    fn apply_update(
        &mut self,
        outcome: Result<CalendarEventUpdateOutcome, CalendarWorkflowError>,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        let update_sync_required = matches!(
            &outcome,
            Ok(CalendarEventUpdateOutcome::ConfirmedButNotCached { .. })
                | Err(CalendarWorkflowError::Provider(
                    CalendarOperationError::UpdateOutcomeUnknown
                ))
                | Err(CalendarWorkflowError::Provider(
                    CalendarOperationError::EventChanged
                ))
                | Err(CalendarWorkflowError::InvalidUpdateConfirmation)
                | Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        let status = match outcome {
            Ok(CalendarEventUpdateOutcome::Cached(_)) => EditStatus::Updated,
            Ok(CalendarEventUpdateOutcome::ConfirmedButNotCached { .. }) => {
                EditStatus::UpdatedButNotCached
            }
            Err(CalendarWorkflowError::Provider(CalendarOperationError::UpdateOutcomeUnknown))
            | Err(CalendarWorkflowError::InvalidUpdateConfirmation) => {
                EditStatus::UpdateOutcomeUnknown
            }
            Err(error) => EditStatus::Failed(error),
        };
        self.set_edit_status(status);
        if update_sync_required {
            cx.emit(EventComposerEvent::CalendarWriteSyncRequired);
        }
        cx.emit(EventComposerEvent::CalendarChanged);
        cx.notify();
    }

    fn apply_submission(
        &mut self,
        creation_outcome: Result<CalendarEventCreationOutcome, CalendarWorkflowError>,
        conflict_recovery: Option<
            Result<Option<PendingCalendarEventCreation>, CalendarWorkflowError>,
        >,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        match creation_outcome {
            Err(CalendarWorkflowError::Cache(CalendarCacheError::CreationAlreadyPending)) => {
                self.state =
                    ComposerState::Create(Self::resolve_pending_conflict(conflict_recovery));
            }
            Err(error) if creation_was_not_dispatched(error) => {
                self.state = ComposerState::Create(CreateState::DraftFailed(error));
            }
            creation_outcome => self.apply_creation_outcome(creation_outcome, None),
        }
        cx.emit(EventComposerEvent::PendingOperationUpdated);
        cx.emit(EventComposerEvent::CalendarChanged);
        cx.notify();
    }

    fn apply_recovery(
        &mut self,
        recovery_outcome: Result<CalendarEventCreationOutcome, CalendarWorkflowError>,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        self.apply_creation_outcome(recovery_outcome, None);
        cx.emit(EventComposerEvent::PendingOperationUpdated);
        cx.emit(EventComposerEvent::CalendarChanged);
        cx.notify();
    }

    fn apply_creation_outcome(
        &mut self,
        creation_outcome: Result<CalendarEventCreationOutcome, CalendarWorkflowError>,
        conflict_recovery: Option<
            Result<Option<PendingCalendarEventCreation>, CalendarWorkflowError>,
        >,
    ) {
        let Some(creation) = self
            .state
            .pending_creation()
            .map(|(creation, _)| creation.clone())
        else {
            return;
        };
        self.state = ComposerState::Create(match creation_outcome {
            Ok(CalendarEventCreationOutcome::Cached(_)) => {
                CreateState::Completed(CreationOutcome::Confirmed)
            }
            Ok(CalendarEventCreationOutcome::ConfirmedButNotCached { .. }) => {
                CreateState::Pending {
                    creation,
                    status: PendingCreationStatus::ConfirmedButNotCached,
                }
            }
            Ok(CalendarEventCreationOutcome::Rejected) => {
                CreateState::Completed(CreationOutcome::Rejected)
            }
            Ok(CalendarEventCreationOutcome::RejectedButPending { cache_error }) => {
                CreateState::Pending {
                    creation,
                    status: PendingCreationStatus::RejectedCleanupFailed(
                        CalendarWorkflowError::Cache(cache_error),
                    ),
                }
            }
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::CreationOutcomeUnknown,
            )) => CreateState::Pending {
                creation,
                status: PendingCreationStatus::StillUnresolved,
            },
            Err(CalendarWorkflowError::Cache(CalendarCacheError::CreationAlreadyPending)) => {
                Self::resolve_pending_conflict(conflict_recovery)
            }
            Err(error) => CreateState::Pending {
                creation,
                status: PendingCreationStatus::Failed(error),
            },
        });
    }

    fn resolve_pending_conflict(
        conflict_recovery: Option<
            Result<Option<PendingCalendarEventCreation>, CalendarWorkflowError>,
        >,
    ) -> CreateState {
        match conflict_recovery {
            Some(Ok(Some(PendingCalendarEventCreation::Unresolved(creation)))) => {
                CreateState::Pending {
                    creation,
                    status: PendingCreationStatus::Unresolved,
                }
            }
            Some(Ok(Some(PendingCalendarEventCreation::Rejected(creation)))) => {
                CreateState::Pending {
                    creation,
                    status: PendingCreationStatus::RejectedPendingCleanup,
                }
            }
            Some(Ok(None)) | None => CreateState::PendingResolutionRequired,
            Some(Err(error)) => CreateState::PendingResolutionFailed(error),
        }
    }

    fn set_edit_status(&mut self, new_status: EditStatus) {
        if let ComposerState::Edit(EditState::Ready { status, .. }) = &mut self.state {
            *status = new_status;
        }
    }

    fn creation_for_submission(
        &self,
        cx: &App,
    ) -> Option<Result<CalendarEventCreation, DraftError>> {
        match &self.state {
            ComposerState::Create(
                CreateState::Draft | CreateState::DraftFailed(_) | CreateState::InvalidDraft(_),
            ) => Some(self.build_creation(cx)),
            ComposerState::Create(
                CreateState::Pending { .. }
                | CreateState::PendingResolutionRequired
                | CreateState::PendingResolutionFailed(_)
                | CreateState::Completed(_),
            )
            | ComposerState::Edit(_) => None,
        }
    }

    fn build_creation(&self, cx: &App) -> Result<CalendarEventCreation, DraftError> {
        let draft = self.build_draft(cx)?;
        if !super::time::event_endpoints_within_view_week(
            draft.local_start(),
            draft.local_end(),
            self.week.view().range(),
        ) {
            return Err(DraftError::OutsideSelectedWeek);
        }
        CalendarEventCreation::try_new(self.week.view().clone(), draft)
            .map_err(|_| DraftError::InvalidTime)
    }

    fn build_draft(&self, cx: &App) -> Result<CalendarEventDraft, DraftError> {
        let subject = self.event_subject(cx)?;
        let (local_start, local_end) = self.selected_local_times(cx)?;
        let time_zone = self.week.time_zone();
        super::time::create_event_draft(
            subject,
            local_start,
            local_end,
            &time_zone,
            CalendarTimeZoneId::try_from(self.week.time_zone_label().to_owned())
                .expect("the selected time-zone identifier is valid"),
        )
    }

    fn event_subject(&self, cx: &App) -> Result<String, DraftError> {
        let subject = self.subject.read(cx).value().trim().to_owned();
        if subject.is_empty() {
            return Err(DraftError::MissingSubject);
        }
        Ok(subject)
    }

    fn selected_local_times(&self, cx: &App) -> Result<(NaiveDateTime, NaiveDateTime), DraftError> {
        let local_start = super::time::selected_picker_time(&self.start, cx)?;
        let local_end = super::time::selected_picker_time(&self.end, cx)?;
        Ok((local_start, local_end))
    }
}

async fn recover_pending_conflict<S: CalendarSession + ?Sized>(
    session: &S,
    creation: &CalendarEventCreation,
    creation_outcome: &Result<CalendarEventCreationOutcome, CalendarWorkflowError>,
) -> Option<Result<Option<PendingCalendarEventCreation>, CalendarWorkflowError>> {
    if !matches!(
        creation_outcome,
        Err(CalendarWorkflowError::Cache(
            CalendarCacheError::CreationAlreadyPending
        ))
    ) {
        return None;
    }
    Some(
        session
            .load_cached_view(creation.view())
            .await
            .map(|snapshot| snapshot.pending_creation().cloned()),
    )
}

fn creation_was_not_dispatched(error: CalendarWorkflowError) -> bool {
    matches!(
        error,
        CalendarWorkflowError::Cache(_)
            | CalendarWorkflowError::AccountMismatch
            | CalendarWorkflowError::OperationInProgress
            | CalendarWorkflowError::CalendarWriteSyncRequired
            | CalendarWorkflowError::StateUnavailable
    )
}
