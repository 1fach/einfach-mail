use std::sync::Arc;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike, Weekday};
use einfach_mail_application::{
    CalendarCacheError, CalendarEventCreationOutcome, CalendarOperationError, CalendarSession,
    CalendarWorkflowError,
};
use einfach_mail_domain::{
    CalendarEventCreation, CalendarEventDraft, CalendarTimeZoneId, CalendarViewRange,
    CalendarViewReference, PendingCalendarEventCreation,
};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    date_picker::{DatePicker, DatePickerState},
    input::{Input, InputState},
    time_field::TimePrecision,
};
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, Entity, EventEmitter, IntoElement, Render, Role, Task,
    Window, div, prelude::*,
};
use jiff::{Timestamp, civil, tz::TimeZone};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

/// Requests a calendar reload after an event creation attempt.
pub(crate) enum EventComposerEvent {
    /// Reloads cached events and the unresolved creation operation.
    PendingOperationUpdated,
    /// Opens the account's calendar write permission review.
    ReviewCalendarWritePermission,
}

/// Provides the account, view, and local-time context for the composer.
pub(crate) struct EventComposerSetup<S: CalendarSession + ?Sized + 'static> {
    /// Account-scoped calendar session used to stage and create the event.
    pub(super) session: Arc<S>,
    /// Calendar week that must contain both event endpoints.
    pub(super) view: CalendarViewReference,
    /// IANA or UTC time zone used to resolve the selected local times.
    pub(super) time_zone: TimeZone,
    /// User-facing label for the selected time zone.
    pub(super) time_zone_label: String,
    /// Persisted operation to retry with the same creation ID, when present.
    pub(super) pending_creation: Option<PendingCalendarEventCreation>,
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
    creation: Option<CalendarEventCreation>,
    status: CreationStatus,
    task: Option<Task<()>>,
}

#[derive(Clone, Copy)]
enum CreationStatus {
    Editing,
    Pending,
    InvalidDraft(DraftError),
    Submitting,
    Failed(CalendarWorkflowError),
    DiscardingRejected,
    RejectedPendingCleanup(Option<CalendarWorkflowError>),
    Rejected,
    PendingResolutionRequired(Option<CalendarWorkflowError>),
    Confirmed,
    ConfirmedButNotCached,
}

#[derive(Clone, Copy)]
enum DraftError {
    MissingSubject,
    MissingDate,
    InvalidTime,
    AmbiguousLocalTime,
    OutsideSelectedWeek,
}

impl<S: CalendarSession + ?Sized + 'static> EventComposer<S> {
    /// Creates a composer that retries or resolves a saved operation.
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
        let (subject, start, end) = new_composer_inputs(setup.week_start, window, cx);
        let (creation, status) = match setup.pending_creation {
            Some(PendingCalendarEventCreation::Unresolved(creation)) => {
                (Some(creation), CreationStatus::Pending)
            }
            Some(PendingCalendarEventCreation::Rejected(creation)) => {
                (Some(creation), CreationStatus::RejectedPendingCleanup(None))
            }
            None => (None, CreationStatus::Editing),
        };
        Self {
            session: setup.session,
            view: setup.view,
            time_zone: setup.time_zone,
            time_zone_id,
            time_zone_label: setup.time_zone_label,
            subject,
            start,
            end,
            status,
            creation,
            task: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit() {
            return;
        }
        if matches!(self.status, CreationStatus::RejectedPendingCleanup(_)) {
            self.discard_rejected_creation(cx);
            return;
        }
        match self.creation_for_submission(cx) {
            Ok(creation) => self.submit_creation(creation, cx),
            Err(error) => {
                self.show_draft_error(error, cx);
            }
        }
    }

    fn can_submit(&self) -> bool {
        !matches!(
            self.status,
            CreationStatus::Submitting
                | CreationStatus::Confirmed
                | CreationStatus::PendingResolutionRequired(_)
        )
    }

    fn show_draft_error(&mut self, draft_error: DraftError, cx: &mut Context<Self>) {
        self.status = CreationStatus::InvalidDraft(draft_error);
        cx.notify();
    }

    fn submit_creation(&mut self, creation: CalendarEventCreation, cx: &mut Context<Self>) {
        self.creation = Some(creation.clone());
        self.status = CreationStatus::Submitting;
        self.start_submission(creation, cx);
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
                CreationStatus::Confirmed
            }
            Ok(CalendarEventCreationOutcome::ConfirmedButNotCached { .. }) => {
                CreationStatus::ConfirmedButNotCached
            }
            Ok(CalendarEventCreationOutcome::Rejected) => {
                self.creation = None;
                CreationStatus::Rejected
            }
            Ok(CalendarEventCreationOutcome::RejectedButPending { cache_error }) => {
                CreationStatus::RejectedPendingCleanup(Some(CalendarWorkflowError::Cache(
                    cache_error,
                )))
            }
            Err(CalendarWorkflowError::Cache(CalendarCacheError::CreationAlreadyPending)) => {
                self.resolve_pending_conflict(conflict_recovery)
            }
            Err(error) => CreationStatus::Failed(error),
        };
    }

    fn resolve_pending_conflict(
        &mut self,
        conflict_recovery: Option<
            Result<Option<PendingCalendarEventCreation>, CalendarWorkflowError>,
        >,
    ) -> CreationStatus {
        match conflict_recovery {
            Some(Ok(Some(PendingCalendarEventCreation::Unresolved(creation)))) => {
                self.creation = Some(creation);
                CreationStatus::Pending
            }
            Some(Ok(Some(PendingCalendarEventCreation::Rejected(creation)))) => {
                self.creation = Some(creation);
                CreationStatus::RejectedPendingCleanup(None)
            }
            Some(Ok(None)) => {
                self.creation = None;
                CreationStatus::PendingResolutionRequired(None)
            }
            Some(Err(error)) => {
                self.creation = None;
                CreationStatus::PendingResolutionRequired(Some(error))
            }
            None => CreationStatus::PendingResolutionRequired(None),
        }
    }

    fn discard_rejected_creation(&mut self, cx: &mut Context<Self>) {
        let Some(creation) = self.creation.clone() else {
            self.status = CreationStatus::PendingResolutionRequired(None);
            cx.notify();
            return;
        };
        self.status = CreationStatus::DiscardingRejected;
        let session = Arc::clone(&self.session);
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = session.discard_rejected_event_creation(&creation).await;
            let _ = this.update(cx, |composer, cx| {
                composer.task = None;
                match result {
                    Ok(()) => {
                        composer.creation = None;
                        composer.status = CreationStatus::Rejected;
                        cx.emit(EventComposerEvent::PendingOperationUpdated);
                    }
                    Err(error) => {
                        composer.status = CreationStatus::RejectedPendingCleanup(Some(error));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
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
            .map_err(|_| DraftError::OutsideSelectedWeek)
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
        div()
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
            )
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
        let CreationStatus::InvalidDraft(draft_error) = self.status else {
            return None;
        };
        Some(render_draft_error(draft_error, colors).into_any_element())
    }

    fn render_pending(&self, colors: ColorTokens) -> impl IntoElement {
        let creation = self
            .creation
            .as_ref()
            .expect("pending status retains its staged creation");
        div()
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
                    CreationStatus::Submitting | CreationStatus::DiscardingRejected
                ),
                |element| element.child(render_creation_submitting(self.status, colors)),
            )
            .child(render_pending_explanation(self.status, colors))
    }

    fn render_status(&self, colors: ColorTokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.status {
            CreationStatus::InvalidDraft(draft_error) => {
                return Some(render_draft_error(draft_error, colors).into_any_element());
            }
            CreationStatus::PendingResolutionRequired(Some(error)) => {
                return Some(render_pending_resolution_failure(error, colors).into_any_element());
            }
            CreationStatus::PendingResolutionRequired(None) => {
                return Some(render_pending_operation_missing(colors).into_any_element());
            }
            CreationStatus::RejectedPendingCleanup(Some(error)) => {
                return Some(render_rejected_cleanup_failure(error, colors).into_any_element());
            }
            _ => {}
        }
        let status_text = status_text(self.status)?;
        let status_message = render_status_message(status_text, self.status, colors);
        if self.requires_calendar_write_permission_review() {
            Some(render_calendar_permission_review(status_message, cx))
        } else {
            Some(status_message)
        }
    }

    fn requires_calendar_write_permission_review(&self) -> bool {
        matches!(
            self.status,
            CreationStatus::Failed(CalendarWorkflowError::Provider(
                einfach_mail_application::CalendarOperationError::AccessDenied
            ))
        )
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if matches!(
            self.status,
            CreationStatus::Confirmed
                | CreationStatus::Rejected
                | CreationStatus::PendingResolutionRequired(_)
        ) {
            return render_close_action();
        }
        self.render_submit_action(cx)
    }

    fn render_submit_action(&self, cx: &mut Context<Self>) -> AnyElement {
        let retrying = self.creation.is_some();
        let submitting = matches!(self.status, CreationStatus::Submitting);
        Button::new("event-composer-submit")
            .debug_selector(|| "event-composer-submit".to_owned())
            .primary()
            .label(submit_label(self.status, submitting, retrying))
            .loading(submitting || matches!(self.status, CreationStatus::DiscardingRejected))
            .disabled(submitting || matches!(self.status, CreationStatus::DiscardingRejected))
            .on_click(cx.listener(|composer, _, _, cx| composer.submit(cx)))
            .into_any_element()
    }
}

impl<S: CalendarSession + ?Sized + 'static> EventEmitter<EventComposerEvent> for EventComposer<S> {}

impl<S: CalendarSession + ?Sized + 'static> Render for EventComposer<S> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let has_pending_creation = self.creation.is_some();
        div()
            .id("calendar-event-composer")
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .when(has_pending_creation, |element| {
                element.child(self.render_pending(colors))
            })
            .when(!has_pending_creation, |element| {
                element.child(self.render_fields(colors))
            })
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

fn render_pending_subject(creation: &CalendarEventCreation) -> impl IntoElement {
    div()
        .text_sm()
        .font_bold()
        .child(creation.draft().subject().to_owned())
}

fn render_pending_range(creation: &CalendarEventCreation) -> impl IntoElement {
    div().text_sm().child(format_local_range(creation.draft()))
}

fn render_pending_explanation(status: CreationStatus, colors: ColorTokens) -> impl IntoElement {
    let message = if matches!(status, CreationStatus::RejectedPendingCleanup(_)) {
        "Microsoft rejected this event. Clear the saved draft locally before submitting a corrected event."
    } else {
        "This event may already exist in Microsoft. Retrying uses the same operation ID."
    };
    div()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(message)
}

fn render_creation_submitting(status: CreationStatus, colors: ColorTokens) -> impl IntoElement {
    let message = if matches!(status, CreationStatus::DiscardingRejected) {
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
    div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(Role::Alert)
        .text_sm()
        .text_color(colors.destructive)
        .child(format!(
            "Microsoft rejected this event, but its saved draft could not be cleared: {}",
            match error {
                CalendarWorkflowError::Cache(cache_error) => {
                    cache_creation_error_text(cache_error)
                }
                _ => "the calendar recovery action is unavailable.",
            }
        ))
}

fn render_pending_resolution_failure(
    error: CalendarWorkflowError,
    colors: ColorTokens,
) -> impl IntoElement {
    div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(Role::Alert)
        .text_sm()
        .text_color(colors.destructive)
        .child(format!(
            "{} Close this sheet, refresh the calendar, and retry.",
            pending_resolution_error_text(error)
        ))
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
    div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(Role::Alert)
        .text_sm()
        .text_color(colors.destructive)
        .child(
            "Another event operation needs attention. Close this sheet, refresh the calendar, and resolve the saved operation.",
        )
}

fn status_text(status: CreationStatus) -> Option<&'static str> {
    match status {
        CreationStatus::Failed(error) => Some(creation_error_text(error)),
        CreationStatus::Confirmed => Some("Event created and saved to the calendar."),
        CreationStatus::ConfirmedButNotCached => Some(
            "Microsoft confirmed the event, but it could not be saved locally. Retry to recover it safely.",
        ),
        CreationStatus::Rejected => {
            Some("Microsoft rejected this event. Correct the details and submit a new event.")
        }
        CreationStatus::RejectedPendingCleanup(None) => Some(
            "Microsoft rejected this event. Clear the saved draft locally before submitting a corrected event.",
        ),
        CreationStatus::DiscardingRejected => Some("Clearing the rejected event draft…"),
        _ => None,
    }
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

fn render_status_message(
    message: &'static str,
    status: CreationStatus,
    colors: ColorTokens,
) -> AnyElement {
    let color = if matches!(status, CreationStatus::Confirmed) {
        colors.foreground
    } else {
        colors.destructive
    };
    div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(status_role(status))
        .text_sm()
        .text_color(color)
        .child(message)
        .into_any_element()
}

fn status_role(status: CreationStatus) -> Role {
    if matches!(status, CreationStatus::Confirmed) {
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

fn submit_label(status: CreationStatus, submitting: bool, retrying: bool) -> &'static str {
    if submitting {
        "Creating event…"
    } else if matches!(status, CreationStatus::RejectedPendingCleanup(_)) {
        "Clear rejected draft"
    } else if matches!(status, CreationStatus::DiscardingRejected) {
        "Clearing draft…"
    } else if retrying {
        "Retry event creation"
    } else {
        "Create event"
    }
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
    offset_date_time(timestamp)
}

fn offset_date_time(timestamp: Timestamp) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(timestamp.as_second())
        .ok()?
        .replace_nanosecond(u32::try_from(timestamp.subsec_nanosecond()).ok()?)
        .ok()
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
        CalendarWorkflowError::Provider(CalendarOperationError::EventCreationRejected) => {
            "Microsoft rejected this event request. Check the event details and try again."
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

fn cache_creation_error_text(error: CalendarCacheError) -> &'static str {
    match error {
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
        CalendarCacheError::PendingMailSend => {
            "A pending mail send must be resolved before the account cache can be removed."
        }
        CalendarCacheError::UnresolvedEventCreation => {
            "A pending calendar event must be resolved before the account cache can be removed."
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveDateTime};
    use einfach_mail_application::{CalendarCacheError, CalendarWorkflowError};
    use einfach_mail_domain::CalendarViewRange;
    use jiff::tz::TimeZone;
    use time::{Date, Month, PrimitiveDateTime};

    use super::{creation_error_text, event_endpoints_within_view_week, resolve_local_datetime};

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

    fn local_time(date: Date, hour: u8, minute: u8) -> PrimitiveDateTime {
        date.with_hms(hour, minute, 0)
            .expect("synthetic local time is valid")
    }
}
