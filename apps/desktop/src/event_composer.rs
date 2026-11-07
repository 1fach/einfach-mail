use crate::calendar_time::{to_offset_date_time, to_timestamp};
use std::sync::Arc;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike, Weekday};
use einfach_mail_application::{
    CalendarCacheError, CalendarEventCreationOutcome, CalendarEventDeletionOutcome,
    CalendarEventUpdateOutcome, CalendarOperationError, CalendarSession, CalendarWorkflowError,
};
use einfach_mail_domain::{
    CalendarEventCreation, CalendarEventDraft, CalendarEventUpdate, CalendarEventUpdateDraft,
    CalendarOccurrence, CalendarOccurrenceTime, CalendarTimeZoneId, CalendarViewRange,
    CalendarViewReference, EditableCalendarEvent, PendingCalendarEventCreation,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    date_picker::{DatePicker, DatePickerState},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputState},
    time_field::TimePrecision,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, Entity, EventEmitter, IntoElement, KeyBinding, Render,
    Role, Task, Window, div, prelude::*,
};
use jiff::{civil, tz::TimeZone};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

const EVENT_COMPOSER_WRITE_PENDING_CONTEXT: &str = "CalendarEventWritePending";

pub(crate) fn bind_event_composer_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        gpui_kit::NoAction,
        Some(EVENT_COMPOSER_WRITE_PENDING_CONTEXT),
    )]);
}

/// Reports a change made through the event composer.
pub(crate) enum EventComposerEvent {
    /// Reloads cached occurrences after an event write.
    CalendarChanged,
    /// Blocks further event writes until a successful sync reconciles an uncertain outcome.
    CalendarWriteSyncRequired,
    /// Reloads the saved creation operation after a create attempt or cleanup.
    PendingOperationUpdated,
    /// Opens the account's calendar write permission review.
    ReviewCalendarWritePermission,
}

/// Selects a new event, a saved creation retry, or an existing event to change.
pub(super) enum EventComposerIntent {
    /// Creates an event or resumes the account's saved creation.
    Create {
        /// A persisted operation to retry with its existing id, when present.
        pending_creation: Option<PendingCalendarEventCreation>,
    },
    /// Loads a fresh provider snapshot before allowing an edit.
    Edit {
        /// The cached occurrence used to locate the current provider event.
        cached_event: CalendarOccurrence,
    },
}

/// Provides the account, view, and local-time context for the composer.
pub(crate) struct EventComposerSetup<S: CalendarSession + ?Sized + 'static> {
    /// Account-scoped calendar session used to create, update, or delete the event.
    pub(super) session: Arc<S>,
    /// Calendar week that must contain both event endpoints.
    pub(super) view: CalendarViewReference,
    /// IANA or UTC time zone used to resolve the selected local times.
    pub(super) time_zone: TimeZone,
    /// User-facing label for the selected time zone.
    pub(super) time_zone_label: String,
    /// Calendar action the composer should perform.
    pub(super) intent: EventComposerIntent,
    /// First date in the displayed calendar week.
    pub(super) week_start: civil::Date,
}

/// Shows and submits a single timed event for a calendar week.
pub(crate) struct EventComposer<S: CalendarSession + ?Sized + 'static> {
    session: Arc<S>,
    view: CalendarViewReference,
    time_zone: TimeZone,
    time_zone_id: CalendarTimeZoneId,
    time_zone_label: String,
    subject: Entity<InputState>,
    start: Entity<DatePickerState>,
    end: Entity<DatePickerState>,
    mode: ComposerMode,
    creation: Option<CalendarEventCreation>,
    status: ComposerStatus,
    task: Option<Task<()>>,
}

enum ComposerMode {
    Create,
    Edit {
        editable_event: Option<Box<EditableCalendarEvent>>,
    },
}

struct EditableEventFormValues {
    subject: String,
    start: NaiveDateTime,
    end: NaiveDateTime,
}

#[derive(Clone, Copy)]
enum ComposerStatus {
    Editing,
    Pending,
    LoadingEditableEvent,
    InvalidDraft(DraftError),
    Submitting,
    Failed(CalendarWorkflowError),
    PendingResolutionRequired(Option<CalendarWorkflowError>),
    DiscardingRejected,
    RejectedPendingCleanup(Option<CalendarWorkflowError>),
    Rejected,
    Confirmed,
    ConfirmedButNotCached,
    Updated,
    UpdatedButNotCached,
    UpdateOutcomeUnknown,
    Deleting,
    Deleted,
    DeletedButNotCached,
    DeleteOutcomeUnknown,
    DeletionFailed(CalendarWorkflowError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DraftError {
    MissingSubject,
    MissingDate,
    InvalidTime,
    AmbiguousLocalTime,
    OutsideSelectedWeek,
    NoChanges,
}

impl<S: CalendarSession + ?Sized + 'static> EventComposer<S> {
    /// Creates a composer for event creation, recovery, or editing.
    ///
    /// # Panics
    ///
    /// Panics if Jiff's IANA or UTC time-zone identifier violates the domain's
    /// time-zone identifier constraints.
    pub(crate) fn new(
        setup: EventComposerSetup<S>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let time_zone_id =
            CalendarTimeZoneId::try_from(setup.time_zone.iana_name().unwrap_or("UTC").to_owned())
                .expect("the fixed time-zone identifier is valid");
        let (mode, pending_creation, cached_event) = match setup.intent {
            EventComposerIntent::Create { pending_creation } => {
                (ComposerMode::Create, pending_creation, None)
            }
            EventComposerIntent::Edit { cached_event } => (
                ComposerMode::Edit {
                    editable_event: None,
                },
                None,
                Some(cached_event),
            ),
        };
        let (subject, start, end) = new_composer_inputs(setup.week_start, window, cx);
        let (creation, status) = match pending_creation {
            Some(PendingCalendarEventCreation::Unresolved(creation)) => {
                (Some(creation), ComposerStatus::Pending)
            }
            Some(PendingCalendarEventCreation::Rejected(creation)) => {
                (Some(creation), ComposerStatus::RejectedPendingCleanup(None))
            }
            None if cached_event.is_some() => (None, ComposerStatus::LoadingEditableEvent),
            None => (None, ComposerStatus::Editing),
        };
        let status = if cached_event.is_some() {
            ComposerStatus::LoadingEditableEvent
        } else {
            status
        };
        let mut composer = Self {
            session: setup.session,
            view: setup.view,
            time_zone: setup.time_zone,
            time_zone_id,
            time_zone_label: setup.time_zone_label,
            subject,
            start,
            end,
            mode,
            status,
            creation,
            task: None,
        };
        if let Some(cached_event) = cached_event {
            composer.start_edit_load(cached_event, window, cx);
        }
        composer
    }

    fn start_edit_load(
        &mut self,
        cached_event: CalendarOccurrence,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let editable_event = session.editable_event(&cached_event).await;
            let _ = this.update_in(cx, |composer, window, cx| {
                composer.apply_editable_event(editable_event, window, cx);
            });
        }));
    }

    fn apply_editable_event(
        &mut self,
        editable_result: Result<EditableCalendarEvent, CalendarWorkflowError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        match editable_result {
            Ok(editable_event) => self.load_edit_fields(editable_event, window, cx),
            Err(error) => self.status = ComposerStatus::Failed(error),
        }
        cx.notify();
    }

    fn load_edit_fields(
        &mut self,
        editable_event: EditableCalendarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form_values) = editable_event_picker_values(&editable_event, &self.time_zone)
        else {
            self.status = ComposerStatus::Failed(CalendarWorkflowError::InvalidEditableEvent);
            return;
        };
        self.set_edit_form_values(form_values, window, cx);
        self.mode = ComposerMode::Edit {
            editable_event: Some(Box::new(editable_event)),
        };
        self.status = ComposerStatus::Editing;
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

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit() {
            return;
        }
        match &self.mode {
            ComposerMode::Create
                if matches!(self.status, ComposerStatus::RejectedPendingCleanup(_)) =>
            {
                self.discard_rejected_creation(cx);
            }
            ComposerMode::Create => match self.creation_for_submission(cx) {
                Ok(creation) => self.submit_creation(creation, cx),
                Err(error) => self.show_draft_error(error, cx),
            },
            ComposerMode::Edit {
                editable_event: Some(editable_event),
            } => match self.update_for_submission(editable_event, cx) {
                Ok(update) => self.submit_update(update, cx),
                Err(error) => self.show_draft_error(error, cx),
            },
            ComposerMode::Edit {
                editable_event: None,
            } => {}
        }
    }

    fn can_submit(&self) -> bool {
        if matches!(
            self.status,
            ComposerStatus::Submitting
                | ComposerStatus::DiscardingRejected
                | ComposerStatus::LoadingEditableEvent
                | ComposerStatus::Deleting
                | ComposerStatus::Confirmed
                | ComposerStatus::PendingResolutionRequired(_)
        ) {
            return false;
        }
        match &self.mode {
            ComposerMode::Create => true,
            ComposerMode::Edit { editable_event } => {
                editable_event.is_some() && !is_terminal_edit_status(self.status)
            }
        }
    }

    fn show_draft_error(&mut self, draft_error: DraftError, cx: &mut Context<Self>) {
        self.status = ComposerStatus::InvalidDraft(draft_error);
        cx.notify();
    }

    fn submit_creation(&mut self, creation: CalendarEventCreation, cx: &mut Context<Self>) {
        self.creation = Some(creation.clone());
        self.status = ComposerStatus::Submitting;
        self.start_submission(creation, cx);
        cx.notify();
    }

    fn discard_rejected_creation(&mut self, cx: &mut Context<Self>) {
        let Some(creation) = self.creation.clone() else {
            self.status = ComposerStatus::PendingResolutionRequired(None);
            cx.notify();
            return;
        };
        self.status = ComposerStatus::DiscardingRejected;
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = session.discard_rejected_event_creation(&creation).await;
            let _ = this.update(cx, |composer, cx| {
                composer.task = None;
                match result {
                    Ok(()) => {
                        composer.creation = None;
                        composer.status = ComposerStatus::Rejected;
                        cx.emit(EventComposerEvent::PendingOperationUpdated);
                    }
                    Err(error) => {
                        composer.status = ComposerStatus::RejectedPendingCleanup(Some(error));
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
        editable_event: &EditableCalendarEvent,
        cx: &App,
    ) -> Result<CalendarEventUpdate, DraftError> {
        let subject = self.event_subject(cx)?;
        let (selected_start, selected_end) = self.selected_local_times(cx)?;
        let CalendarOccurrenceTime::Timed {
            start: source_start,
            end: source_end,
        } = editable_event.occurrence().time()
        else {
            return Err(DraftError::InvalidTime);
        };
        let start = selected_event_time(selected_start, *source_start, &self.time_zone)?;
        let end = selected_event_time(selected_end, *source_end, &self.time_zone)?;
        let draft = CalendarEventUpdateDraft::timed(subject, start, end)
            .map_err(|_| DraftError::InvalidTime)?;
        CalendarEventUpdate::new(editable_event.clone(), draft).map_err(|_| DraftError::NoChanges)
    }

    pub(crate) fn edit_write_in_progress(&self) -> bool {
        matches!(
            (&self.mode, self.status),
            (
                ComposerMode::Edit {
                    editable_event: Some(_)
                },
                ComposerStatus::Submitting
            )
        ) || matches!(
            (&self.mode, self.status),
            (
                ComposerMode::Edit {
                    editable_event: Some(_)
                },
                ComposerStatus::Deleting
            )
        )
    }

    fn editable_event(&self) -> Option<&EditableCalendarEvent> {
        match &self.mode {
            ComposerMode::Edit { editable_event } => editable_event.as_deref(),
            ComposerMode::Create => None,
        }
    }

    fn confirm_deletion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_delete() {
            return;
        }
        let Some(event) = self.editable_event() else {
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

    fn can_delete(&self) -> bool {
        self.editable_event().is_some()
            && !self.edit_write_in_progress()
            && !is_terminal_edit_status(self.status)
    }

    fn start_deletion(&mut self, cx: &mut Context<Self>) {
        let Some(event) = self.editable_event().cloned() else {
            return;
        };
        if !self.can_delete() {
            return;
        }
        self.status = ComposerStatus::Deleting;
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
        self.status = match outcome {
            Ok(CalendarEventDeletionOutcome::Deleted) => ComposerStatus::Deleted,
            Ok(CalendarEventDeletionOutcome::DeletedButNotCached { .. }) => {
                ComposerStatus::DeletedButNotCached
            }
            Err(CalendarWorkflowError::Provider(CalendarOperationError::DeleteOutcomeUnknown)) => {
                ComposerStatus::DeleteOutcomeUnknown
            }
            Err(error) => ComposerStatus::DeletionFailed(error),
        };
        if sync_required {
            cx.emit(EventComposerEvent::CalendarWriteSyncRequired);
        }
        cx.emit(EventComposerEvent::CalendarChanged);
        cx.notify();
    }

    fn submit_update(&mut self, update: CalendarEventUpdate, cx: &mut Context<Self>) {
        self.status = ComposerStatus::Submitting;
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
        self.status = match outcome {
            Ok(CalendarEventUpdateOutcome::Cached(_)) => ComposerStatus::Updated,
            Ok(CalendarEventUpdateOutcome::ConfirmedButNotCached { .. }) => {
                ComposerStatus::UpdatedButNotCached
            }
            Err(CalendarWorkflowError::Provider(CalendarOperationError::UpdateOutcomeUnknown))
            | Err(CalendarWorkflowError::InvalidUpdateConfirmation) => {
                ComposerStatus::UpdateOutcomeUnknown
            }
            Err(error) => ComposerStatus::Failed(error),
        };
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
        self.apply_creation_outcome(creation_outcome, conflict_recovery);
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
        self.status = match creation_outcome {
            Ok(CalendarEventCreationOutcome::Cached(_)) => {
                self.creation = None;
                ComposerStatus::Confirmed
            }
            Ok(CalendarEventCreationOutcome::ConfirmedButNotCached { .. }) => {
                ComposerStatus::ConfirmedButNotCached
            }
            Ok(CalendarEventCreationOutcome::Rejected) => {
                self.creation = None;
                ComposerStatus::Rejected
            }
            Ok(CalendarEventCreationOutcome::RejectedButPending { cache_error }) => {
                ComposerStatus::RejectedPendingCleanup(Some(CalendarWorkflowError::Cache(
                    cache_error,
                )))
            }
            Err(CalendarWorkflowError::Cache(CalendarCacheError::CreationAlreadyPending)) => {
                self.resolve_pending_conflict(conflict_recovery)
            }
            Err(error) => ComposerStatus::Failed(error),
        };
    }

    fn resolve_pending_conflict(
        &mut self,
        conflict_recovery: Option<
            Result<Option<PendingCalendarEventCreation>, CalendarWorkflowError>,
        >,
    ) -> ComposerStatus {
        match conflict_recovery {
            Some(Ok(Some(PendingCalendarEventCreation::Unresolved(creation)))) => {
                self.creation = Some(creation);
                ComposerStatus::Pending
            }
            Some(Ok(Some(PendingCalendarEventCreation::Rejected(creation)))) => {
                self.creation = Some(creation);
                ComposerStatus::RejectedPendingCleanup(None)
            }
            Some(Ok(None)) => {
                self.creation = None;
                ComposerStatus::PendingResolutionRequired(None)
            }
            Some(Err(error)) => {
                self.creation = None;
                ComposerStatus::PendingResolutionRequired(Some(error))
            }
            None => ComposerStatus::PendingResolutionRequired(None),
        }
    }

    fn creation_for_submission(&self, cx: &App) -> Result<CalendarEventCreation, DraftError> {
        if let Some(pending_creation) = &self.creation {
            return Ok(pending_creation.clone());
        }
        self.build_creation(cx)
    }

    fn build_creation(&self, cx: &App) -> Result<CalendarEventCreation, DraftError> {
        let draft = self.build_draft(cx)?;
        if !event_endpoints_within_view_week(
            draft.local_start(),
            draft.local_end(),
            self.view.range(),
        ) {
            return Err(DraftError::OutsideSelectedWeek);
        }
        CalendarEventCreation::try_new(self.view.clone(), draft)
            .map_err(|_| DraftError::InvalidTime)
    }

    fn build_draft(&self, cx: &App) -> Result<CalendarEventDraft, DraftError> {
        let subject = self.event_subject(cx)?;
        let (local_start, local_end) = self.selected_local_times(cx)?;
        create_event_draft(
            subject,
            local_start,
            local_end,
            &self.time_zone,
            self.time_zone_id.clone(),
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
        let local_start = selected_picker_time(&self.start, cx)?;
        let local_end = selected_picker_time(&self.end, cx)?;
        Ok((local_start, local_end))
    }

    fn render_fields(&self, colors: ColorTokens) -> impl IntoElement {
        let fields = div()
            .id("event-composer-fields")
            .debug_selector(|| "event-composer-fields".to_owned())
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_subject_field())
            .child(self.render_start_field())
            .child(self.render_end_field())
            .child(self.render_time_zone(colors))
            .when_some(
                self.render_draft_validation(colors),
                |element, validation| element.child(validation),
            );
        #[cfg(test)]
        let fields = fields.test_support();
        fields
    }

    fn render_subject_field(&self) -> impl IntoElement {
        div()
            .id("event-composer-subject")
            .debug_selector(|| "event-composer-subject".to_owned())
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().font_bold().child("Title"))
            .child(
                div()
                    .id("event-composer-subject-input")
                    .debug_selector(|| "event-composer-subject-input".to_owned())
                    .child(Input::new(&self.subject).w_full()),
            )
    }

    fn render_start_field(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().font_bold().child("Starts"))
            .child(DatePicker::new(&self.start).w_full())
    }

    fn render_end_field(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().font_bold().child("Ends"))
            .child(DatePicker::new(&self.end).w_full())
    }

    fn render_time_zone(&self, colors: ColorTokens) -> impl IntoElement {
        div()
            .id("event-composer-time-zone")
            .role(Role::Status)
            .text_sm()
            .text_color(colors.muted_foreground)
            .child(format!("Times use {0}.", self.time_zone_label))
    }

    fn render_draft_validation(&self, colors: ColorTokens) -> Option<AnyElement> {
        let ComposerStatus::InvalidDraft(draft_error) = self.status else {
            return None;
        };
        Some(render_draft_error(draft_error, colors).into_any_element())
    }

    fn render_pending(&self, colors: ColorTokens) -> impl IntoElement {
        let creation = self
            .creation
            .as_ref()
            .expect("pending status retains its staged creation");
        let pending = div()
            .id("event-creation-pending")
            .debug_selector(|| "event-creation-pending".to_owned())
            .role(Role::Status)
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .rounded_lg()
            .child(render_pending_subject(creation))
            .child(render_pending_range(creation))
            .when(
                matches!(
                    self.status,
                    ComposerStatus::Submitting | ComposerStatus::DiscardingRejected
                ),
                |element| element.child(render_creation_submitting(self.status, colors)),
            )
            .child(render_pending_explanation(self.status, colors));
        #[cfg(test)]
        let pending = pending.test_support();
        pending
    }

    fn render_status(&self, colors: ColorTokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.status {
            ComposerStatus::InvalidDraft(draft_error) => {
                return Some(render_draft_error(draft_error, colors).into_any_element());
            }
            ComposerStatus::PendingResolutionRequired(Some(error)) => {
                return Some(render_pending_resolution_failure(error, colors).into_any_element());
            }
            ComposerStatus::PendingResolutionRequired(None) => {
                return Some(render_pending_operation_missing(colors).into_any_element());
            }
            ComposerStatus::RejectedPendingCleanup(Some(error)) => {
                return Some(render_rejected_cleanup_failure(error, colors).into_any_element());
            }
            _ => {}
        }
        let status_text = match &self.mode {
            ComposerMode::Create => creation_status_text(self.status),
            ComposerMode::Edit { .. } => match self.status {
                ComposerStatus::Deleting
                | ComposerStatus::Deleted
                | ComposerStatus::DeletedButNotCached
                | ComposerStatus::DeleteOutcomeUnknown
                | ComposerStatus::DeletionFailed(_) => deletion_status_text(self.status),
                _ => update_status_text(self.status),
            },
        }?;
        let message = render_status_message(status_text, self.status, colors);
        if self.requires_calendar_write_permission_review() {
            Some(render_calendar_permission_review(message, cx))
        } else {
            Some(message)
        }
    }

    fn requires_calendar_write_permission_review(&self) -> bool {
        matches!(
            self.status,
            ComposerStatus::Failed(CalendarWorkflowError::Provider(
                CalendarOperationError::AccessDenied
            )) | ComposerStatus::DeletionFailed(CalendarWorkflowError::Provider(
                CalendarOperationError::AccessDenied
            ))
        )
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.action_is_terminal() {
            return render_close_action();
        }
        if matches!(&self.mode, ComposerMode::Edit { .. }) {
            let actions = div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    Button::new("event-composer-delete")
                        .debug_selector(|| "event-composer-delete".to_owned())
                        .danger()
                        .label("Delete…")
                        .disabled(!self.can_delete())
                        .on_click(cx.listener(|composer, _, window, cx| {
                            composer.confirm_deletion(window, cx);
                        })),
                )
                .child(self.render_submit_action(cx));
            #[cfg(test)]
            let actions = actions.id("event-composer-actions").test_support();
            actions.into_any_element()
        } else {
            self.render_submit_action(cx)
        }
    }

    fn action_is_terminal(&self) -> bool {
        matches!(
            self.status,
            ComposerStatus::Confirmed
                | ComposerStatus::Rejected
                | ComposerStatus::Updated
                | ComposerStatus::UpdatedButNotCached
                | ComposerStatus::UpdateOutcomeUnknown
                | ComposerStatus::PendingResolutionRequired(_)
        ) || matches!(
            &self.mode,
            ComposerMode::Edit {
                editable_event: None
            }
        ) || matches!(&self.mode, ComposerMode::Edit { .. }) && is_terminal_edit_status(self.status)
    }

    fn render_submit_action(&self, cx: &mut Context<Self>) -> AnyElement {
        let submitting = matches!(
            self.status,
            ComposerStatus::Submitting | ComposerStatus::DiscardingRejected
        );
        Button::new("event-composer-submit")
            .debug_selector(|| "event-composer-submit".to_owned())
            .primary()
            .label(self.submit_label())
            .loading(submitting)
            .disabled(submitting || !self.can_submit())
            .on_click(cx.listener(|composer, _, _, cx| composer.submit(cx)))
            .into_any_element()
    }

    fn submit_label(&self) -> &'static str {
        match (&self.mode, self.status) {
            (ComposerMode::Edit { .. }, ComposerStatus::Submitting) => "Saving changes…",
            (ComposerMode::Edit { .. }, _) => "Save changes",
            (ComposerMode::Create, ComposerStatus::DiscardingRejected) => "Clearing draft…",
            (ComposerMode::Create, ComposerStatus::RejectedPendingCleanup(_)) => {
                "Clear rejected draft"
            }
            (ComposerMode::Create, ComposerStatus::Submitting) => "Creating event…",
            (ComposerMode::Create, _) if self.creation.is_some() => "Retry event creation",
            (ComposerMode::Create, _) => "Create event",
        }
    }
}

impl<S: CalendarSession + ?Sized + 'static> EventEmitter<EventComposerEvent> for EventComposer<S> {}

impl<S: CalendarSession + ?Sized + 'static> Render for EventComposer<S> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let has_pending_creation = self.creation.is_some();
        let write_in_progress = self.edit_write_in_progress();
        let show_fields = match &self.mode {
            ComposerMode::Create => !has_pending_creation,
            ComposerMode::Edit { editable_event } => {
                editable_event.is_some() && !is_terminal_edit_status(self.status)
            }
        };
        div()
            .id("calendar-event-composer")
            .when(write_in_progress, |element| {
                element.key_context(EVENT_COMPOSER_WRITE_PENDING_CONTEXT)
            })
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .when(has_pending_creation, |element| {
                element.child(self.render_pending(colors))
            })
            .when(!has_pending_creation, |element| {
                element.when(show_fields, |element| {
                    element.child(self.render_fields(colors))
                })
            })
            .when(
                matches!(self.status, ComposerStatus::LoadingEditableEvent),
                |element| element.child(render_edit_loading(colors)),
            )
            .when_some(self.render_status(colors, cx), |element, status| {
                element.child(status)
            })
            .child(self.render_actions(cx))
    }
}

fn picker_value(date: civil::Date, hour: u32, minute: u32) -> Option<NaiveDateTime> {
    let year = i32::from(date.year());
    NaiveDate::from_ymd_opt(
        year,
        u32::try_from(date.month()).ok()?,
        u32::try_from(date.day()).ok()?,
    )?
    .and_hms_opt(hour, minute, 0)
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

fn create_event_draft(
    subject: String,
    local_start: NaiveDateTime,
    local_end: NaiveDateTime,
    time_zone: &TimeZone,
    time_zone_id: CalendarTimeZoneId,
) -> Result<CalendarEventDraft, DraftError> {
    let domain_start = domain_datetime(local_start).ok_or(DraftError::InvalidTime)?;
    let domain_end = domain_datetime(local_end).ok_or(DraftError::InvalidTime)?;
    let start =
        resolve_local_datetime(local_start, time_zone).ok_or(DraftError::AmbiguousLocalTime)?;
    let end = resolve_local_datetime(local_end, time_zone).ok_or(DraftError::AmbiguousLocalTime)?;
    CalendarEventDraft::timed(subject, domain_start, domain_end, time_zone_id, start, end)
        .map_err(|_| DraftError::InvalidTime)
}

fn event_endpoints_within_view_week(
    local_start: PrimitiveDateTime,
    local_end: PrimitiveDateTime,
    range: &CalendarViewRange,
) -> bool {
    [local_start.date(), local_end.date()]
        .into_iter()
        .all(|event_date| {
            range.start_date() <= event_date && event_date < range.end_date_exclusive()
        })
}

fn selected_picker_time(
    picker: &Entity<DatePickerState>,
    cx: &App,
) -> Result<NaiveDateTime, DraftError> {
    picker
        .read(cx)
        .date_time()
        .start()
        .ok_or(DraftError::MissingDate)
}

fn editable_event_picker_values(
    event: &EditableCalendarEvent,
    time_zone: &TimeZone,
) -> Option<EditableEventFormValues> {
    let CalendarOccurrenceTime::Timed { start, end } = event.occurrence().time() else {
        return None;
    };
    Some(EditableEventFormValues {
        subject: event.occurrence().subject().unwrap_or_default().to_owned(),
        start: picker_value_for_offset(*start, time_zone)?,
        end: picker_value_for_offset(*end, time_zone)?,
    })
}

fn picker_value_for_offset(value: OffsetDateTime, time_zone: &TimeZone) -> Option<NaiveDateTime> {
    let timestamp = to_timestamp(value)?;
    let local_datetime = timestamp.to_zoned(time_zone.clone()).datetime();
    picker_value(
        local_datetime.date(),
        u32::try_from(local_datetime.hour()).ok()?,
        u32::try_from(local_datetime.minute()).ok()?,
    )
}

fn selected_event_time(
    selected: NaiveDateTime,
    original: OffsetDateTime,
    time_zone: &TimeZone,
) -> Result<OffsetDateTime, DraftError> {
    if picker_value_for_offset(original, time_zone) == Some(selected) {
        return Ok(original);
    }
    resolve_local_datetime(selected, time_zone).ok_or(DraftError::AmbiguousLocalTime)
}

fn render_pending_subject(creation: &CalendarEventCreation) -> impl IntoElement {
    div()
        .text_sm()
        .font_bold()
        .child(creation.draft().subject().to_owned())
}

fn render_pending_range(creation: &CalendarEventCreation) -> impl IntoElement {
    div().text_sm().child(format_local_range(creation.draft()))
}

fn render_pending_explanation(status: ComposerStatus, colors: ColorTokens) -> impl IntoElement {
    let message = match status {
        ComposerStatus::RejectedPendingCleanup(Some(_)) => {
            "The rejected event stays saved until local cleanup succeeds."
        }
        ComposerStatus::RejectedPendingCleanup(None) => {
            "Microsoft rejected this event. Clear the saved draft locally before submitting a corrected event."
        }
        ComposerStatus::DiscardingRejected => "Clearing the rejected draft locally…",
        _ => "This event may already exist in Microsoft. Retrying uses the same operation ID.",
    };
    div()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(message)
}

fn render_creation_submitting(status: ComposerStatus, colors: ColorTokens) -> impl IntoElement {
    let message = if matches!(status, ComposerStatus::DiscardingRejected) {
        "Clearing rejected event draft…"
    } else {
        "Creating event…"
    };
    div()
        .id("event-creation-submitting")
        .debug_selector(|| "event-creation-submitting".to_owned())
        .role(Role::Status)
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(message)
}

fn render_rejected_cleanup_failure(
    error: CalendarWorkflowError,
    colors: ColorTokens,
) -> impl IntoElement {
    let status = div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(Role::Alert)
        .text_sm()
        .text_color(colors.destructive)
        .child(format!(
            "Microsoft rejected this event, but its saved draft could not be cleared: {}",
            match error {
                CalendarWorkflowError::Cache(cache_error) => {
                    rejected_cleanup_error_text(cache_error)
                }
                _ => "the calendar recovery action is unavailable.",
            }
        ));
    #[cfg(test)]
    let status = status.test_support();
    status
}

fn rejected_cleanup_error_text(error: CalendarCacheError) -> &'static str {
    match error {
        CalendarCacheError::InUse => {
            "The calendar cache is in use by another operation. Wait for it to finish, then clear the rejected draft."
        }
        CalendarCacheError::Unavailable => {
            "The encrypted calendar cache is unavailable. Restore cache access, then clear the rejected draft."
        }
        CalendarCacheError::AccountMismatch => {
            "The rejected draft belongs to another account. Return to that account before clearing it."
        }
        CalendarCacheError::InvalidData | CalendarCacheError::CreationAlreadyPending => {
            "The saved event draft could not be verified. Refresh the calendar before clearing it."
        }
    }
}

fn render_pending_resolution_failure(
    error: CalendarWorkflowError,
    colors: ColorTokens,
) -> impl IntoElement {
    let status = div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(Role::Alert)
        .text_sm()
        .text_color(colors.destructive)
        .child(format!(
            "{} Close this sheet, refresh the calendar, and retry.",
            pending_resolution_error_text(error)
        ));
    #[cfg(test)]
    let status = status.test_support();
    status
}

fn pending_resolution_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::Cache(CalendarCacheError::Unavailable) => {
            "The encrypted calendar cache is unavailable."
        }
        CalendarWorkflowError::Cache(CalendarCacheError::AccountMismatch) => {
            "The saved event operation belongs to another account."
        }
        _ => "The saved event operation could not be loaded.",
    }
}

fn render_pending_operation_missing(colors: ColorTokens) -> impl IntoElement {
    let status = div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(Role::Alert)
        .text_sm()
        .text_color(colors.destructive)
        .child(
            "Another event operation needs attention. Close this sheet, refresh the calendar, and resolve the saved operation.",
        );
    #[cfg(test)]
    let status = status.test_support();
    status
}

fn creation_status_text(status: ComposerStatus) -> Option<&'static str> {
    match status {
        ComposerStatus::Failed(error) => Some(creation_error_text(error)),
        ComposerStatus::Confirmed => Some("Event created and saved to the calendar."),
        ComposerStatus::ConfirmedButNotCached => Some(
            "Microsoft confirmed the event, but it could not be saved locally. Retry to recover it safely.",
        ),
        ComposerStatus::Rejected => {
            Some("Microsoft rejected this event. Correct the details and submit a new event.")
        }
        ComposerStatus::RejectedPendingCleanup(_) => None,
        ComposerStatus::DiscardingRejected => Some("Clearing the rejected event draft…"),
        _ => None,
    }
}

fn update_status_text(status: ComposerStatus) -> Option<&'static str> {
    match status {
        ComposerStatus::Failed(error) => Some(update_error_text(error)),
        ComposerStatus::Updated => Some("Event changes were saved to the calendar."),
        ComposerStatus::UpdatedButNotCached => Some(
            "Microsoft confirmed the changes, but they could not be saved locally. Sync each calendar week you opened this session before editing another event.",
        ),
        ComposerStatus::UpdateOutcomeUnknown => Some(
            "Microsoft may have applied these changes. Sync each calendar week you opened this session before editing another event.",
        ),
        _ => None,
    }
}

fn deletion_status_text(status: ComposerStatus) -> Option<&'static str> {
    match status {
        ComposerStatus::Deleting => Some("Deleting the appointment…"),
        ComposerStatus::Deleted => Some("Appointment deleted from the calendar."),
        ComposerStatus::DeletedButNotCached => Some(
            "Microsoft confirmed the deletion, but the local calendar could not be updated. Sync each calendar week you opened this session before making another calendar change.",
        ),
        ComposerStatus::DeleteOutcomeUnknown => Some(
            "Microsoft may have deleted this appointment. Sync each calendar week you opened this session before making another calendar change.",
        ),
        ComposerStatus::DeletionFailed(error) => Some(deletion_error_text(error)),
        _ => None,
    }
}

fn deletion_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::Provider(CalendarOperationError::EventDeletionRejected) => {
            "Microsoft rejected the deletion. The appointment was not removed."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::EventChanged) => {
            "This appointment changed while the editor was open. Sync each calendar week you opened this session, then review it again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::EventNotEditable)
        | CalendarWorkflowError::InvalidEditableEvent => {
            "Microsoft did not confirm that this appointment can be deleted here."
        }
        CalendarWorkflowError::CalendarWriteSyncRequired => {
            "Return to the calendar and sync each week you opened this session before deleting this appointment."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::AccessDenied) => {
            "Microsoft denied access to delete this appointment. Review the requested permission or your organization's consent policy."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and connect again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::RateLimited { .. }) => {
            "Microsoft is limiting calendar requests. Wait a moment, then try again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::ServiceUnavailable) => {
            "The calendar service is unavailable. Check your connection, then try again."
        }
        CalendarWorkflowError::OperationInProgress => {
            "The calendar is syncing. Try deleting this appointment when sync finishes."
        }
        _ => "The appointment could not be deleted. Check the calendar and try again.",
    }
}

fn render_status_message(
    message: &'static str,
    status: ComposerStatus,
    colors: ColorTokens,
) -> AnyElement {
    let color = if matches!(status, ComposerStatus::Confirmed | ComposerStatus::Updated) {
        colors.foreground
    } else {
        colors.destructive
    };
    let status = div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(status_role(status))
        .text_sm()
        .text_color(color)
        .child(message);
    #[cfg(test)]
    let status = status.test_support();
    status.into_any_element()
}

fn render_calendar_permission_review<S: CalendarSession + ?Sized + 'static>(
    status_message: AnyElement,
    cx: &mut Context<EventComposer<S>>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(status_message)
        .child("Calendars.ReadWrite lets Einfach Mail create, read, update, and delete calendar events. Reviewing it does not repeat the failed event action.")
        .child(
            Button::new("review-calendar-write-permission")
                .debug_selector(|| "review-calendar-write-permission".to_owned())
                .ghost()
                .label("Review Calendars.ReadWrite…")
                .on_click(cx.listener(|_, _, window, cx| {
                    cx.emit(EventComposerEvent::ReviewCalendarWritePermission);
                    window.defer(cx, |window, cx| {
                        gpui_kit::component::WindowExt::close_sheet(window, cx);
                    });
                })),
        )
        .into_any_element()
}

fn status_role(status: ComposerStatus) -> Role {
    if matches!(
        status,
        ComposerStatus::Confirmed | ComposerStatus::Updated | ComposerStatus::Deleted
    ) {
        Role::Status
    } else {
        Role::Alert
    }
}

fn render_close_action() -> AnyElement {
    Button::new("event-composer-close")
        .debug_selector(|| "event-composer-close".to_owned())
        .label("Done")
        .on_click(|_, window, cx| {
            gpui_kit::component::WindowExt::close_sheet(window, cx);
        })
        .into_any_element()
}

fn is_terminal_edit_status(status: ComposerStatus) -> bool {
    matches!(
        status,
        ComposerStatus::Updated
            | ComposerStatus::UpdatedButNotCached
            | ComposerStatus::UpdateOutcomeUnknown
            | ComposerStatus::Deleted
            | ComposerStatus::DeletedButNotCached
            | ComposerStatus::DeleteOutcomeUnknown
            | ComposerStatus::DeletionFailed(
                CalendarWorkflowError::InvalidEditableEvent
                    | CalendarWorkflowError::CalendarWriteSyncRequired
                    | CalendarWorkflowError::Provider(
                        CalendarOperationError::EventChanged
                            | CalendarOperationError::EventNotEditable
                            | CalendarOperationError::DeleteOutcomeUnknown
                    )
            )
            | ComposerStatus::Failed(
                CalendarWorkflowError::InvalidEditableEvent
                    | CalendarWorkflowError::InvalidUpdateConfirmation
                    | CalendarWorkflowError::CalendarWriteSyncRequired
                    | CalendarWorkflowError::Provider(
                        CalendarOperationError::EventChanged
                            | CalendarOperationError::EventNotEditable
                            | CalendarOperationError::UpdateOutcomeUnknown
                    )
            )
    )
}

fn render_edit_loading(colors: ColorTokens) -> impl IntoElement {
    div()
        .id("event-edit-loading")
        .debug_selector(|| "event-edit-loading".to_owned())
        .role(Role::Status)
        .text_sm()
        .text_color(colors.muted_foreground)
        .child("Checking event details…")
}

fn new_date_picker<S: CalendarSession + ?Sized + 'static>(
    selected_date_time: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<EventComposer<S>>,
) -> Entity<DatePickerState> {
    cx.new(|cx| {
        let mut state = DatePickerState::new(window, cx)
            .time_precision(TimePrecision::Minute)
            .first_day_of_week(Weekday::Mon);
        state.set_date_time(selected_date_time, window, cx);
        state
    })
}

fn new_composer_inputs<S: CalendarSession + ?Sized + 'static>(
    week_start: civil::Date,
    window: &mut Window,
    cx: &mut Context<EventComposer<S>>,
) -> (
    Entity<InputState>,
    Entity<DatePickerState>,
    Entity<DatePickerState>,
) {
    let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Event title"));
    let start = picker_value(week_start, 9, 0).unwrap_or_else(|| fallback_picker_value(9, 0));
    let end = picker_value(week_start, 9, 30).unwrap_or_else(|| fallback_picker_value(9, 30));
    (
        subject,
        new_date_picker(start, window, cx),
        new_date_picker(end, window, cx),
    )
}

fn fallback_picker_value(hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1970, 1, 5)
        .expect("the fallback calendar date is valid")
        .and_hms_opt(hour, minute, 0)
        .expect("the fallback calendar time is valid")
}

fn domain_datetime(local_date_time: NaiveDateTime) -> Option<PrimitiveDateTime> {
    let month = Month::try_from(u8::try_from(local_date_time.month()).ok()?).ok()?;
    let date = Date::from_calendar_date(
        local_date_time.year(),
        month,
        u8::try_from(local_date_time.day()).ok()?,
    )
    .ok()?;
    let time = Time::from_hms_nano(
        u8::try_from(local_date_time.hour()).ok()?,
        u8::try_from(local_date_time.minute()).ok()?,
        u8::try_from(local_date_time.second()).ok()?,
        local_date_time.nanosecond(),
    )
    .ok()?;
    Some(PrimitiveDateTime::new(date, time))
}

fn resolve_local_datetime(
    local_date_time: NaiveDateTime,
    time_zone: &TimeZone,
) -> Option<OffsetDateTime> {
    let date_time = civil::DateTime::new(
        i16::try_from(local_date_time.year()).ok()?,
        i8::try_from(local_date_time.month()).ok()?,
        i8::try_from(local_date_time.day()).ok()?,
        i8::try_from(local_date_time.hour()).ok()?,
        i8::try_from(local_date_time.minute()).ok()?,
        i8::try_from(local_date_time.second()).ok()?,
        i32::try_from(local_date_time.nanosecond()).ok()?,
    )
    .ok()?;
    let timestamp = time_zone
        .to_ambiguous_zoned(date_time)
        .unambiguous()
        .ok()?
        .timestamp();
    to_offset_date_time(timestamp)
}

fn format_local_range(draft: &CalendarEventDraft) -> String {
    format!(
        "{} – {} ({})",
        format_local_date_time(draft.local_start()),
        format_local_date_time(draft.local_end()),
        draft.time_zone().as_str()
    )
}

fn format_local_date_time(local_date_time: PrimitiveDateTime) -> String {
    let month = u32::from(local_date_time.month() as u8);
    NaiveDate::from_ymd_opt(
        local_date_time.year(),
        month,
        u32::from(local_date_time.day()),
    )
    .and_then(|date| {
        date.and_hms_opt(
            u32::from(local_date_time.hour()),
            u32::from(local_date_time.minute()),
            0,
        )
    })
    .map(|resolved_date_time| resolved_date_time.format("%a, %b %-d · %H:%M").to_string())
    .unwrap_or_else(|| local_date_time.to_string())
}

fn render_draft_error(error: DraftError, colors: ColorTokens) -> impl IntoElement {
    let message = match error {
        DraftError::MissingSubject => "Enter an event title.",
        DraftError::MissingDate => "Choose both start and end dates.",
        DraftError::InvalidTime => "The end must be later than the start.",
        DraftError::AmbiguousLocalTime => {
            "This local time does not exist or occurs twice because of a clock change. Choose another time."
        }
        DraftError::OutsideSelectedWeek => {
            "Choose start and end dates inside the week currently shown in the calendar."
        }
        DraftError::NoChanges => "Change the title or event time before saving.",
    };
    div()
        .id("event-composer-validation")
        .debug_selector(|| "event-composer-validation".to_owned())
        .role(Role::Alert)
        .text_sm()
        .text_color(colors.destructive)
        .child(message)
}

fn creation_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::Provider(
            einfach_mail_application::CalendarOperationError::AccessDenied,
        ) => {
            "Microsoft denied access to create this calendar event. Review the requested permission or your organization's consent policy."
        }
        CalendarWorkflowError::Provider(
            einfach_mail_application::CalendarOperationError::SignInRequired,
        ) => "Your Microsoft sign-in needs attention. Return to Accounts and connect again.",
        CalendarWorkflowError::Provider(
            einfach_mail_application::CalendarOperationError::RateLimited { .. },
        ) => "Microsoft is limiting calendar requests. Wait a moment, then retry this event.",
        CalendarWorkflowError::Provider(
            einfach_mail_application::CalendarOperationError::ServiceUnavailable,
        ) => "The calendar service is unavailable. Check your connection, then retry this event.",
        CalendarWorkflowError::OperationInProgress => {
            "The calendar is syncing. Retry this event when sync finishes."
        }
        CalendarWorkflowError::InvalidConfirmation => {
            "Microsoft returned an event that did not match this draft. Retry the saved operation to check the same event."
        }
        CalendarWorkflowError::Cache(error) => cache_creation_error_text(error),
        _ => "The event could not be confirmed. Retry the saved operation to check the same event.",
    }
}

fn update_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::CalendarWriteSyncRequired => {
            "Return to the calendar and sync each week you opened this session before editing this event."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::EventNotEditable) => {
            "Microsoft did not confirm that this event is a standalone appointment that can be edited here."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::EventChanged) => {
            "This event changed while the editor was open. Close this sheet, sync each calendar week you opened this session, and open Edit again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::UpdateOutcomeUnknown) => {
            "Microsoft may have applied these changes. Sync each calendar week you opened this session before editing another event."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::AccessDenied) => {
            "Microsoft denied access to update this event. Review the requested permission or your organization's consent policy."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and connect again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::RateLimited { .. }) => {
            "Microsoft is limiting calendar requests. Wait a moment, then save again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::ServiceUnavailable) => {
            "The calendar service is unavailable. Check your connection, then try again."
        }
        CalendarWorkflowError::OperationInProgress => {
            "The calendar is syncing. Save these changes after sync finishes."
        }
        CalendarWorkflowError::InvalidEditableEvent => {
            "The calendar service returned event details that cannot be edited safely."
        }
        CalendarWorkflowError::InvalidUpdateConfirmation => {
            "Microsoft returned an update that did not match the requested changes. Sync each calendar week you opened this session before editing another event."
        }
        _ => "The event could not be updated. Check the calendar and try again.",
    }
}

fn cache_creation_error_text(error: CalendarCacheError) -> &'static str {
    match error {
        CalendarCacheError::InUse => {
            "The calendar cache is in use by another operation. This event was not sent to Microsoft. Wait for it to finish, then retry."
        }
        CalendarCacheError::Unavailable => {
            "The encrypted calendar cache is unavailable. This event was not sent to Microsoft. Restore cache access, then retry."
        }
        CalendarCacheError::AccountMismatch => {
            "This calendar cache belongs to another account. Return to the matching account before retrying."
        }
        CalendarCacheError::InvalidData => {
            "The event draft could not be saved safely. Microsoft has not received it. Check the event details and retry."
        }
        CalendarCacheError::CreationAlreadyPending => {
            "Another event creation needs to be resolved before a new event can be created."
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveDateTime};
    use einfach_mail_application::{CalendarCacheError, CalendarWorkflowError};
    use einfach_mail_domain::CalendarViewRange;
    use jiff::tz::TimeZone;
    use time::{Date, Month, PrimitiveDateTime, Time};

    use super::{
        DraftError, creation_error_text, event_endpoints_within_view_week, picker_value_for_offset,
        resolve_local_datetime, selected_event_time,
    };

    fn local_datetime(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, day)
            .expect("synthetic date is valid")
            .and_hms_opt(hour, minute, 0)
            .expect("synthetic local time is valid")
    }

    #[test]
    fn local_time_resolution_rejects_daylight_saving_gaps_and_folds() {
        let new_york =
            TimeZone::get("America/New_York").expect("the synthetic Linux IANA zone exists");

        assert!(resolve_local_datetime(local_datetime(2030, 3, 10, 2, 30), &new_york).is_none());
        assert!(resolve_local_datetime(local_datetime(2030, 11, 3, 1, 30), &new_york).is_none());
        assert!(resolve_local_datetime(local_datetime(2030, 3, 10, 3, 30), &new_york).is_some());
    }

    #[test]
    fn event_endpoints_must_stay_inside_the_half_open_calendar_week() {
        let week_start = Date::from_calendar_date(2030, Month::January, 7)
            .expect("synthetic week start is valid");
        let week_end = Date::from_calendar_date(2030, Month::January, 14)
            .expect("synthetic exclusive week end is valid");
        let range = CalendarViewRange::new(
            week_start.midnight().assume_utc(),
            week_end.midnight().assume_utc(),
            week_start,
            week_end,
        )
        .expect("synthetic calendar week is valid");
        let first_instant = local_time(week_start, 0, 0);
        let last_instant = local_time(
            Date::from_calendar_date(2030, Month::January, 13)
                .expect("synthetic final week date is valid"),
            23,
            59,
        );

        assert!(event_endpoints_within_view_week(
            first_instant,
            last_instant,
            &range
        ));
        assert!(!event_endpoints_within_view_week(
            local_time(
                Date::from_calendar_date(2030, Month::January, 6)
                    .expect("synthetic prior date is valid"),
                23,
                59,
            ),
            first_instant,
            &range,
        ));
        assert!(!event_endpoints_within_view_week(
            first_instant,
            local_time(week_end, 0, 0),
            &range,
        ));
    }

    #[test]
    fn cache_staging_errors_explain_that_the_event_was_not_sent() {
        assert_eq!(
            creation_error_text(CalendarWorkflowError::Cache(
                CalendarCacheError::Unavailable
            )),
            "The encrypted calendar cache is unavailable. This event was not sent to Microsoft. Restore cache access, then retry."
        );
        assert_eq!(
            creation_error_text(CalendarWorkflowError::Cache(
                CalendarCacheError::InvalidData
            )),
            "The event draft could not be saved safely. Microsoft has not received it. Check the event details and retry."
        );
    }

    #[test]
    fn an_unchanged_minute_preserves_hidden_event_seconds() {
        let time_zone = TimeZone::UTC;
        let original = Date::from_calendar_date(2030, Month::January, 8)
            .expect("synthetic date is valid")
            .with_time(Time::from_hms_nano(9, 30, 45, 123_000_000).expect("valid time"))
            .assume_utc();
        let selected = picker_value_for_offset(original, &time_zone)
            .expect("synthetic time is representable by the minute picker");

        assert_eq!(
            selected_event_time(selected, original, &time_zone),
            Ok(original)
        );
    }

    #[test]
    fn a_changed_local_time_resolves_against_the_selected_zone() {
        let time_zone = TimeZone::get("America/New_York")
            .expect("the synthetic IANA zone exists in Linux zoneinfo");
        let original = Date::from_calendar_date(2030, Month::January, 8)
            .expect("synthetic date is valid")
            .with_time(Time::from_hms(14, 0, 0).expect("valid time"))
            .assume_utc();
        let selected = local_datetime(2030, 1, 8, 10, 0);

        let updated = selected_event_time(selected, original, &time_zone)
            .expect("synthetic local time resolves");

        assert_eq!(updated.unix_timestamp(), original.unix_timestamp() + 3_600);
    }

    #[test]
    fn an_edit_to_a_daylight_saving_gap_is_rejected() {
        let time_zone = TimeZone::get("America/New_York")
            .expect("the synthetic IANA zone exists in Linux zoneinfo");
        let original = Date::from_calendar_date(2030, Month::March, 10)
            .expect("synthetic date is valid")
            .with_time(Time::from_hms(6, 30, 0).expect("valid time"))
            .assume_utc();

        assert_eq!(
            selected_event_time(local_datetime(2030, 3, 10, 2, 30), original, &time_zone),
            Err(DraftError::AmbiguousLocalTime)
        );
    }

    fn local_time(date: Date, hour: u8, minute: u8) -> PrimitiveDateTime {
        date.with_hms(hour, minute, 0)
            .expect("synthetic local time is valid")
    }
}
