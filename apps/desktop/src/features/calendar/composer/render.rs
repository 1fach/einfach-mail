use super::copy::*;
use super::state::*;
use super::time::*;
use super::*;

impl<S: CalendarSession + ?Sized + 'static> EventComposer<S> {
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
            .child(format!("Times use {0}.", self.week.time_zone_label()))
    }

    fn render_draft_validation(&self, colors: ColorTokens) -> Option<AnyElement> {
        let draft_error = self.state.validation_error()?;
        Some(render_draft_error(draft_error, colors).into_any_element())
    }

    fn render_pending(
        &self,
        creation: &CalendarEventCreation,
        status: PendingCreationStatus,
        colors: ColorTokens,
    ) -> impl IntoElement {
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
                    status,
                    PendingCreationStatus::Submitting | PendingCreationStatus::DiscardingRejected
                ),
                |element| element.child(render_creation_submitting(status, colors)),
            )
            .child(render_pending_explanation(status, colors));
        #[cfg(test)]
        let pending = pending.test_support();
        pending
    }

    fn render_status(&self, colors: ColorTokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(status) = self
            .render_draft_validation(colors)
            .or_else(|| self.render_recovery_status(colors))
        {
            return Some(status);
        }
        let status_text = self.state.status_text()?;
        let message = render_status_message(status_text, self.state.status_style(), colors);
        if self.state.requires_calendar_write_permission_review() {
            Some(render_calendar_permission_review(message, cx))
        } else {
            Some(message)
        }
    }

    fn render_recovery_status(&self, colors: ColorTokens) -> Option<AnyElement> {
        match &self.state {
            ComposerState::Create(CreateState::PendingResolutionFailed(error)) => {
                Some(render_pending_resolution_failure(*error, colors).into_any_element())
            }
            ComposerState::Create(CreateState::PendingResolutionRequired) => {
                Some(render_pending_operation_missing(colors).into_any_element())
            }
            ComposerState::Create(CreateState::Pending {
                status: PendingCreationStatus::RejectedCleanupFailed(error),
                ..
            }) => Some(render_rejected_cleanup_failure(*error, colors).into_any_element()),
            _ => None,
        }
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.state.action_is_terminal() {
            return render_close_action();
        }
        if matches!(&self.state, ComposerState::Edit(_)) {
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
                        .disabled(!self.state.can_delete())
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

    fn render_submit_action(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(label) = self.state.submit_label() else {
            return render_close_action();
        };
        Button::new("event-composer-submit")
            .debug_selector(|| "event-composer-submit".to_owned())
            .primary()
            .label(label)
            .loading(self.state.submit_is_loading())
            .disabled(!self.state.can_submit())
            .on_click(cx.listener(|composer, _, _, cx| composer.submit(cx)))
            .into_any_element()
    }
}

impl<S: CalendarSession + ?Sized + 'static> Render for EventComposer<S> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let write_in_progress = self.edit_write_in_progress();
        let pending_creation = self.state.pending_creation();
        div()
            .id("calendar-event-composer")
            .when(write_in_progress, |element| {
                element.key_context(EVENT_COMPOSER_WRITE_PENDING_CONTEXT)
            })
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .when_some(pending_creation, |element, (creation, status)| {
                element.child(self.render_pending(creation, status, colors))
            })
            .when(pending_creation.is_none(), |element| {
                element.when(self.state.show_fields(), |element| {
                    element.child(self.render_fields(colors))
                })
            })
            .when(
                matches!(&self.state, ComposerState::Edit(EditState::Loading)),
                |element| element.child(render_edit_loading(colors)),
            )
            .when_some(self.render_status(colors, cx), |element, status| {
                element.child(status)
            })
            .child(self.render_actions(cx))
    }
}

pub(super) fn render_pending_subject(creation: &CalendarEventCreation) -> impl IntoElement {
    div()
        .text_sm()
        .font_bold()
        .child(creation.draft().subject().to_owned())
}

pub(super) fn render_pending_range(creation: &CalendarEventCreation) -> impl IntoElement {
    div().text_sm().child(format_local_range(creation.draft()))
}

pub(super) fn render_pending_explanation(
    status: PendingCreationStatus,
    colors: ColorTokens,
) -> impl IntoElement {
    let message = match status {
        PendingCreationStatus::RejectedCleanupFailed(_) => {
            "The rejected event stays saved until local cleanup succeeds."
        }
        PendingCreationStatus::RejectedPendingCleanup => {
            "Microsoft rejected this event. Clear the saved draft locally before submitting a corrected event."
        }
        PendingCreationStatus::DiscardingRejected => "Clearing the rejected draft locally…",
        _ => "This event may already exist in Microsoft. Retrying uses the same operation ID.",
    };
    div()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(message)
}

pub(super) fn render_creation_submitting(
    status: PendingCreationStatus,
    colors: ColorTokens,
) -> impl IntoElement {
    let message = if matches!(status, PendingCreationStatus::DiscardingRejected) {
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

pub(super) fn render_rejected_cleanup_failure(
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

pub(super) fn render_pending_resolution_failure(
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

pub(super) fn render_pending_operation_missing(colors: ColorTokens) -> impl IntoElement {
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

pub(super) fn render_status_message(
    message: &'static str,
    style: StatusStyle,
    colors: ColorTokens,
) -> AnyElement {
    let (role, color) = match style {
        StatusStyle::Success => (Role::Status, colors.foreground),
        StatusStyle::DestructiveStatus => (Role::Status, colors.destructive),
        StatusStyle::Error => (Role::Alert, colors.destructive),
    };
    let status = div()
        .id("event-creation-status")
        .debug_selector(|| "event-creation-status".to_owned())
        .role(role)
        .text_sm()
        .text_color(color)
        .child(message);
    #[cfg(test)]
    let status = status.test_support();
    status.into_any_element()
}

pub(super) fn render_calendar_permission_review<S: CalendarSession + ?Sized + 'static>(
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

pub(super) fn render_close_action() -> AnyElement {
    Button::new("event-composer-close")
        .debug_selector(|| "event-composer-close".to_owned())
        .label("Done")
        .on_click(|_, window, cx| {
            gpui_kit::component::WindowExt::close_sheet(window, cx);
        })
        .into_any_element()
}

pub(super) fn render_edit_loading(colors: ColorTokens) -> impl IntoElement {
    div()
        .id("event-edit-loading")
        .debug_selector(|| "event-edit-loading".to_owned())
        .role(Role::Status)
        .text_sm()
        .text_color(colors.muted_foreground)
        .child("Checking event details…")
}

pub(super) fn new_date_picker<S: CalendarSession + ?Sized + 'static>(
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

pub(super) fn new_composer_inputs<S: CalendarSession + ?Sized + 'static>(
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

pub(super) fn render_draft_error(error: DraftError, colors: ColorTokens) -> impl IntoElement {
    let message = draft_error_text(error);
    div()
        .id("event-composer-validation")
        .debug_selector(|| "event-composer-validation".to_owned())
        .role(Role::Alert)
        .text_sm()
        .text_color(colors.destructive)
        .child(message)
}
