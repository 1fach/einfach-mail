use einfach_mail_application::{CalendarSession, CalendarWorkflowError, CalendarWriteReadiness};
use einfach_mail_domain::{CalendarAction, CalendarOccurrenceTime};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{AnyElement, Context, IntoElement, Render, Role, Window, div, prelude::*, rems};
use jiff::civil;

use super::CalendarView;
use super::actions::CALENDAR_EVENT_LIST_CONTEXT;
use super::copy::{
    UTC_FALLBACK_STATUS_LABEL, calendar_error_message, calendar_event_accessible_label, date_label,
    elapsed_label, week_label, write_readiness_message,
};
use super::state::{
    AgendaEvent, AgendaEventKey, AgendaOptionPosition, AgendaRow, CalendarStatus, DisplayEvent,
};

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    fn render_header(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .border_b_1()
            .border_color(colors.border)
            .child(self.render_title_bar(cx))
            .child(self.render_week_navigation(cx))
            .child(self.render_status(colors))
            .when(self.week.is_utc_fallback(), |element| {
                element.child(render_utc_fallback(colors))
            })
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut title_bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(self.render_calendar_title());
        if self.profile.capabilities().calendar().is_read_only() {
            let read_only_status = div()
                .id("calendar-read-only-status")
                .debug_selector(|| "calendar-read-only-status".to_owned())
                .role(Role::Status)
                .aria_label("Calendar is read-only")
                .flex_shrink_0()
                .text_sm()
                .text_color(cx.theme().semantic_tokens().colors.muted_foreground)
                .child("Read-only");
            #[cfg(test)]
            let read_only_status = read_only_status.test_support();
            title_bar = title_bar.child(read_only_status);
        }
        if let Some(selected_event_action) = self.render_selected_event_action(cx) {
            title_bar = title_bar.child(selected_event_action);
        }
        if self.supports_calendar_action(CalendarAction::CreateEvent) {
            title_bar = title_bar.child(self.render_event_button(cx));
        }
        title_bar.child(self.render_sync_button(cx))
    }

    fn render_calendar_title(&self) -> impl IntoElement {
        div()
            .flex_1()
            .min_w(rems(12.))
            .text_lg()
            .font_bold()
            .truncate()
            .child(format!("Calendar · {}", self.profile.label()))
    }

    fn render_sync_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .debug_selector(|| "calendar-sync".to_owned())
            .child(
                Button::new("calendar-sync")
                    .label("Sync now")
                    .loading(self.status.is_sync_active())
                    .disabled(self.status.is_sync_active())
                    .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
            )
    }

    fn render_event_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .debug_selector(|| "calendar-new-event".to_owned())
            .child(
                Button::new("calendar-new-event")
                    .label(if self.pending_creation.is_some() {
                        "Resolve pending event…"
                    } else {
                        "New event…"
                    })
                    .disabled(
                        self.status.is_sync_active()
                            || self.write_readiness != CalendarWriteReadiness::Ready,
                    )
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.open_event_composer(window, cx);
                    })),
            )
    }

    fn render_week_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .debug_selector(|| "calendar-week-navigation".to_owned())
            .child(Self::previous_week_button(cx))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(10.))
                    .text_base()
                    .font_bold()
                    .truncate()
                    .debug_selector(|| "calendar-week-label".to_owned())
                    .child(week_label(self.week.start())),
            )
            .child(Self::next_week_button(cx))
    }

    fn previous_week_button(cx: &mut Context<Self>) -> impl IntoElement {
        week_navigation_button("calendar-previous-week", "Previous week", -1, cx)
    }

    fn next_week_button(cx: &mut Context<Self>) -> impl IntoElement {
        week_navigation_button("calendar-next-week", "Next week", 1, cx)
    }

    fn render_status(&self, colors: ColorTokens) -> AnyElement {
        if let CalendarStatus::Failed(error) = self.status {
            return self.render_sync_error(error, colors);
        }
        self.render_sync_progress(colors)
    }

    fn render_sync_error(&self, error: CalendarWorkflowError, colors: ColorTokens) -> AnyElement {
        let mut status = div()
            .id("calendar-sync-error")
            .debug_selector(|| "calendar-sync-error".to_owned())
            .role(Role::Alert)
            .text_sm()
            .text_color(colors.destructive)
            .child(calendar_error_message(error, !self.events.is_empty()));
        if let Some(message) = write_readiness_message(self.write_readiness) {
            status = status.child(message);
        }
        #[cfg(test)]
        let status = status.test_support();
        status.into_any_element()
    }

    fn render_sync_progress(&self, colors: ColorTokens) -> AnyElement {
        if matches!(self.status, CalendarStatus::UpToDate)
            && let Some(message) = write_readiness_message(self.write_readiness)
        {
            return render_status_label(message.to_owned(), Role::Alert, colors);
        }
        let (label, role) = match self.status {
            CalendarStatus::Idle => ("Calendar is ready to sync".to_owned(), Role::Status),
            CalendarStatus::LoadingCache => {
                ("Loading saved calendar events…".to_owned(), Role::Status)
            }
            CalendarStatus::Syncing => ("Refreshing calendar events…".to_owned(), Role::Status),
            CalendarStatus::UpToDate => match self
                .sync_state
                .as_ref()
                .and_then(|state| state.last_synchronized_at())
            {
                Some(last_sync) => (elapsed_label(last_sync), Role::Status),
                None => ("Calendar is ready.".to_owned(), Role::Status),
            },
            CalendarStatus::Failed(error) => return self.render_sync_error(error, colors),
        };
        render_status_label(label, role, colors)
    }

    fn render_agenda(&self, colors: ColorTokens, cx: &mut Context<Self>) -> AnyElement {
        let agenda = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(primary_calendar_heading());
        let rows = self.agenda_rows();
        let event_count = rows
            .iter()
            .filter(|row| matches!(row, AgendaRow::Event(_)))
            .count();
        if event_count == 0 {
            return agenda
                .child(render_empty_agenda(self.status, colors))
                .into_any_element();
        }

        let mut event_position = 0;
        let event_list = div()
            .id("calendar-event-list")
            .debug_selector(|| "calendar-event-list".to_owned())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .track_scroll(&self.agenda_scroll)
            .key_context(CALENDAR_EVENT_LIST_CONTEXT)
            .role(Role::ListBox)
            .aria_label("Events this week");
        #[cfg(test)]
        let event_list = event_list.test_support();
        let mut event_list = event_list
            .track_focus(&self.event_list_focus)
            .on_action(cx.listener(Self::on_select_previous_calendar_event))
            .on_action(cx.listener(Self::on_select_next_calendar_event))
            .on_action(cx.listener(Self::on_select_first_calendar_event))
            .on_action(cx.listener(Self::on_select_last_calendar_event))
            .on_action(cx.listener(Self::on_open_selected_calendar_event))
            .focus_visible(|style| style.border_1().border_color(colors.ring))
            .overflow_y_scroll();

        for row in &rows {
            event_list = match row {
                AgendaRow::DayHeading(date) => event_list.child(render_agenda_day_heading(*date)),
                AgendaRow::EmptyDay => event_list.child(render_empty_day(colors)),
                AgendaRow::Event(event) => {
                    event_position += 1;
                    event_list.child(render_agenda_event(
                        event,
                        AgendaOptionPosition {
                            event_position,
                            event_count,
                            selected: self.selected_agenda_event.as_ref() == Some(&event.key),
                        },
                        colors,
                        cx,
                    ))
                }
            };
        }
        agenda
            .child(event_list.vertical_scrollbar(&self.agenda_scroll))
            .into_any_element()
    }

    fn render_selected_event_action(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let action = self.selected_event_action()?;
        let cached_event = self.selected_calendar_event()?;
        if !matches!(cached_event.time(), CalendarOccurrenceTime::Timed { .. }) {
            return None;
        }

        Some(
            Button::new("calendar-event-open")
                .debug_selector(|| "calendar-event-open".to_owned())
                .ghost()
                .flex_shrink_0()
                .label(action.label())
                .disabled(
                    self.status.is_sync_active()
                        || self.write_readiness != CalendarWriteReadiness::Ready,
                )
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.open_selected_event(cached_event.clone(), window, cx);
                }))
                .into_any_element(),
        )
    }
}

impl<S> Render for CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id("calendar-view")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_header(colors, cx))
            .child(self.render_agenda(colors, cx))
    }
}

fn week_navigation_button<S>(
    id: &'static str,
    label: &'static str,
    week_offset: i64,
    cx: &mut Context<CalendarView<S>>,
) -> impl IntoElement
where
    S: CalendarSession + ?Sized + 'static,
{
    div()
        .flex()
        .flex_shrink_0()
        .debug_selector(move || id.to_owned())
        .child(
            Button::new(id)
                .ghost()
                .label(label)
                .on_click(cx.listener(move |view, _, _, cx| view.change_week(week_offset, cx))),
        )
}

fn render_utc_fallback(colors: ColorTokens) -> impl IntoElement {
    let status = div()
        .id("calendar-utc-fallback")
        .debug_selector(|| "calendar-utc-fallback".to_owned())
        .role(Role::Status)
        .aria_label(UTC_FALLBACK_STATUS_LABEL)
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(UTC_FALLBACK_STATUS_LABEL);
    #[cfg(test)]
    let status = status.test_support();
    status
}

fn primary_calendar_heading() -> impl IntoElement {
    div().text_sm().font_bold().child("Primary calendar")
}

fn render_empty_agenda(status: CalendarStatus, colors: ColorTokens) -> impl IntoElement {
    let message = match status {
        CalendarStatus::Idle => "Sync this calendar to load events for this week.",
        CalendarStatus::LoadingCache | CalendarStatus::Syncing => "Loading events for this week…",
        CalendarStatus::Failed(_) => "No cached events are available for this week.",
        CalendarStatus::UpToDate => "No events this week.",
    };
    div()
        .id("calendar-empty-state")
        .role(Role::Status)
        .p_4()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(message)
}

fn render_status_label(label: String, role: Role, colors: ColorTokens) -> AnyElement {
    div()
        .id("calendar-sync-status")
        .role(role)
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(label)
        .into_any_element()
}

fn render_agenda_day_heading(date: civil::Date) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .text_sm()
        .font_bold()
        .child(date_label(date))
}

fn render_empty_day(colors: ColorTokens) -> impl IntoElement {
    div()
        .w_full()
        .p_3()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child("No events")
}

fn render_agenda_event<S>(
    agenda_event: &AgendaEvent<'_>,
    position: AgendaOptionPosition,
    colors: ColorTokens,
    cx: &mut Context<CalendarView<S>>,
) -> impl IntoElement
where
    S: CalendarSession + ?Sized + 'static,
{
    let selected = position.selected;
    let event_key = agenda_event.key.clone();
    let date_id = event_key.date.strftime("%Y-%m-%d").to_string();
    let event_id = event_key.event_id.as_str().to_owned();
    let accessible_label = calendar_event_accessible_label(&agenda_event.display, event_key.date);
    let card = div()
        .id(format!("calendar-event-option-{date_id}-{event_id}"))
        .debug_selector(move || format!("calendar-event-row-{date_id}-{event_id}"))
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .bg(if selected {
            colors.accent
        } else {
            colors.surface
        })
        .text_color(if selected {
            colors.accent_foreground
        } else {
            colors.foreground
        })
        .border_1()
        .border_color(if selected {
            colors.accent
        } else {
            colors.border
        })
        .rounded_md()
        .role(Role::ListBoxOption)
        .aria_label(accessible_label)
        .aria_position_in_set(position.event_position)
        .aria_size_of_set(position.event_count)
        .aria_selected(selected)
        .when(selected, |option| option.aria_active_descendant())
        .hover(|option| {
            option.bg(if selected {
                colors.accent
            } else {
                colors.muted
            })
        })
        .child(render_event_heading(
            &agenda_event.display,
            &agenda_event.key,
            colors,
            selected,
        ));
    #[cfg(test)]
    let card = card.test_support();
    card.on_click(cx.listener(move |view, _, window, cx| {
        view.select_event_from_option(event_key.clone(), window, cx);
    }))
}

fn render_event_heading(
    event: &DisplayEvent<'_>,
    key: &AgendaEventKey,
    colors: ColorTokens,
    selected: bool,
) -> impl IntoElement {
    let event_identity = format!(
        "{}-{}",
        key.date.strftime("%Y-%m-%d"),
        key.event_id.as_str()
    );
    let details_selector = format!("calendar-event-details-{event_identity}");
    let time_selector = format!("calendar-event-time-{event_identity}");
    let mut details = div()
        .debug_selector(move || details_selector.clone())
        .flex_1()
        .min_w(rems(10.))
        .flex()
        .flex_col()
        .gap_1()
        .child(render_event_subject(event));
    if let Some(location) = event.event.location() {
        details = details.child(render_event_location(location, colors, selected));
    }
    div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap_3()
        .child(render_event_time(event, time_selector))
        .child(details)
}

fn render_event_time(event: &DisplayEvent<'_>, selector: String) -> impl IntoElement {
    div()
        .debug_selector(move || selector.clone())
        .w_32()
        .flex_shrink_0()
        .text_sm()
        .font_medium()
        .child(event.time_label.clone())
}

fn render_event_subject(event: &DisplayEvent<'_>) -> impl IntoElement {
    let subject = event.event.subject().unwrap_or("(No title)");
    div().font_medium().child(subject.to_owned())
}

fn render_event_location(location: &str, colors: ColorTokens, selected: bool) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .text_sm()
        .text_color(if selected {
            colors.accent_foreground
        } else {
            colors.muted_foreground
        })
        .child(location.to_owned())
}
