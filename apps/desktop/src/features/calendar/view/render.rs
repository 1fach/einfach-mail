use einfach_mail_application::{CalendarSession, CalendarWriteReadiness};
use einfach_mail_domain::CalendarOccurrenceTime;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Icon, StyledExt,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
    v_flex,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, Context, Div, IntoElement, Render, Role, Stateful, Window, div, prelude::*, rems,
};

use crate::components::{
    AppIcon, GroupHeader, InfoBar, OptionFill, PaneHeader, SetPosition, StatusTone,
    filled_list_option, list_option,
};

use super::CalendarView;
use super::actions::CALENDAR_EVENT_LIST_CONTEXT;
use super::copy::{
    UTC_FALLBACK_STATUS_LABEL, calendar_error_message, calendar_event_accessible_label, date_label,
    elapsed_label, week_title, write_readiness_message,
};
use super::state::{
    AgendaEvent, AgendaEventKey, CalendarLayout, CalendarStatus, DisplayEvent, WeekAgenda,
};

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let caption = (self.layout == CalendarLayout::Grid)
            .then(|| self.render_sync_caption(cx.theme().semantic_tokens().colors))
            .flatten();
        PaneHeader::new("calendar-week-label", week_title(self.week.start()))
            .when(
                self.profile.capabilities().calendar().is_read_only(),
                |header| {
                    header.tag_status(
                        "calendar-read-only-status",
                        "Calendar is read-only",
                        "Read-only",
                    )
                },
            )
            .when_some(caption, |header, caption| header.status(caption))
            .when_some(self.render_selected_event_action(cx), |header, action| {
                header.action(action)
            })
            .action(self.render_today_button(cx))
            .action(week_navigation_button(
                IconControl {
                    id: "calendar-previous-week",
                    label: "Previous week",
                    icon: AppIcon::ChevronLeft,
                },
                -1,
                cx,
            ))
            .action(week_navigation_button(
                IconControl {
                    id: "calendar-next-week",
                    label: "Next week",
                    icon: AppIcon::ChevronRight,
                },
                1,
                cx,
            ))
            .action(self.render_sync_button(cx))
    }

    fn render_today_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .debug_selector(|| "calendar-today".to_owned())
            .child(
                Button::new("calendar-today")
                    .ghost()
                    .label("Today")
                    .disabled(self.shows_today())
                    .on_click(cx.listener(|view, _, _, cx| view.show_today(cx))),
            )
    }

    fn render_sync_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .debug_selector(|| "calendar-sync".to_owned())
            .child(
                IconControl {
                    id: "calendar-sync",
                    label: "Sync",
                    icon: AppIcon::Sync,
                }
                .button()
                .loading(self.status.is_sync_active())
                .disabled(self.status.is_sync_active())
                .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
            )
    }

    /// The short sync state beside the title. A failure has its own notice, so it has no caption.
    fn render_sync_caption(&self, colors: ColorTokens) -> Option<AnyElement> {
        let label = match self.status {
            CalendarStatus::Idle => "Calendar is ready to sync".to_owned(),
            CalendarStatus::LoadingCache => "Loading saved calendar events…".to_owned(),
            CalendarStatus::Syncing => "Refreshing calendar events…".to_owned(),
            CalendarStatus::UpToDate => match self
                .sync_state
                .as_ref()
                .and_then(|state| state.last_synchronized_at())
            {
                Some(last_sync) => elapsed_label(last_sync),
                None => "Calendar is ready.".to_owned(),
            },
            CalendarStatus::Failed(_) => return None,
        };
        Some(
            div()
                .id("calendar-sync-status")
                .role(Role::Status)
                .aria_label(label.clone())
                .text_sm()
                .text_color(colors.muted_foreground)
                .truncate()
                .child(label)
                .into_any_element(),
        )
    }

    /// The sync state on its own line under the header, where the agenda layout has no room for
    /// it beside the title.
    fn render_agenda_caption(&self, colors: ColorTokens) -> Option<AnyElement> {
        if self.layout != CalendarLayout::Agenda {
            return None;
        }
        self.render_sync_caption(colors).map(|caption| {
            div()
                .flex_shrink_0()
                .px_4()
                .pt_2()
                .child(caption)
                .into_any_element()
        })
    }

    /// The notices about this week: a failed sync, unsynchronized writes and a missing time zone.
    fn render_notices(&self) -> Option<AnyElement> {
        let mut notices = Vec::new();
        match self.status {
            CalendarStatus::Failed(error) => {
                let mut message = calendar_error_message(error, !self.events.is_empty());
                if let Some(readiness) = write_readiness_message(self.write_readiness) {
                    message.push(' ');
                    message.push_str(readiness);
                }
                notices.push(InfoBar::alert("calendar-sync-error", message).into_any_element());
            }
            CalendarStatus::UpToDate => {
                if let Some(readiness) = write_readiness_message(self.write_readiness) {
                    notices.push(
                        InfoBar::alert("calendar-write-notice", readiness)
                            .tone(StatusTone::Warning)
                            .into_any_element(),
                    );
                }
            }
            CalendarStatus::Idle | CalendarStatus::LoadingCache | CalendarStatus::Syncing => {}
        }
        if self.week.is_utc_fallback() {
            notices.push(
                InfoBar::status("calendar-utc-fallback", UTC_FALLBACK_STATUS_LABEL)
                    .tone(StatusTone::Warning)
                    .accessible_label(UTC_FALLBACK_STATUS_LABEL)
                    .into_any_element(),
            );
        }
        (!notices.is_empty()).then(|| {
            v_flex()
                .w_full()
                .flex_shrink_0()
                .gap_2()
                .px_4()
                .pt_3()
                .children(notices)
                .into_any_element()
        })
    }

    fn render_week(&self, cx: &mut Context<Self>) -> AnyElement {
        let agenda = self.week_agenda();
        match self.layout {
            CalendarLayout::Grid => self.render_week_grid(&agenda, cx),
            CalendarLayout::Agenda => self.render_agenda(&agenda, cx),
        }
    }

    fn render_agenda(&self, agenda: &WeekAgenda<'_>, cx: &mut Context<Self>) -> AnyElement {
        let content = if agenda.events.is_empty() {
            render_empty_agenda(self.status, cx.theme().semantic_tokens().colors).into_any_element()
        } else {
            self.render_agenda_list(agenda, cx)
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(content)
            .into_any_element()
    }

    fn render_agenda_list(&self, agenda: &WeekAgenda<'_>, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().semantic_tokens().colors;
        let mut children = Vec::new();
        for day in &agenda.grid.days {
            children.push(
                GroupHeader::new(
                    format!("calendar-day-header-{}", day.date.strftime("%Y-%m-%d")),
                    date_label(day.date),
                )
                .into_any_element(),
            );
            if day.events.is_empty() {
                children.push(render_empty_day(colors).into_any_element());
            }
            for event_index in day.events.clone() {
                if let Some(option) =
                    self.render_week_event(agenda, event_index, EventPresentation::Row, cx)
                {
                    children.push(div().w_full().px_2().child(option).into_any_element());
                }
            }
        }
        self.render_event_listbox(
            |listbox| listbox.flex_1().min_h_0().flex().flex_col().gap_1().py_1(),
            children,
            cx,
        )
    }

    /// The one listbox of the week. Both layouts fill it with the events in the same flat
    /// order, so the Tab stop, the keys and the set positions do not depend on the layout.
    pub(super) fn render_event_listbox(
        &self,
        arrange: impl FnOnce(Stateful<Div>) -> Stateful<Div>,
        children: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ring = cx.theme().ring;
        let listbox = div()
            .id("calendar-event-list")
            .debug_selector(|| "calendar-event-list".to_owned())
            .track_scroll(&self.agenda_scroll)
            .key_context(CALENDAR_EVENT_LIST_CONTEXT)
            .role(Role::ListBox)
            .aria_label("Events this week");
        let listbox = arrange(listbox);
        #[cfg(test)]
        let listbox = listbox.test_support();
        listbox
            .track_focus(&self.event_list_focus)
            .on_action(cx.listener(Self::on_select_previous_calendar_event))
            .on_action(cx.listener(Self::on_select_next_calendar_event))
            .on_action(cx.listener(Self::on_select_first_calendar_event))
            .on_action(cx.listener(Self::on_select_last_calendar_event))
            .on_action(cx.listener(Self::on_open_selected_calendar_event))
            .focus_visible(move |style| style.border_1().border_color(ring))
            .overflow_y_scroll()
            .children(children)
            .vertical_scrollbar(&self.agenda_scroll)
            .into_any_element()
    }

    /// The option for the event at `event_index` of the flat order, in `presentation`.
    pub(super) fn render_week_event(
        &self,
        agenda: &WeekAgenda<'_>,
        event_index: usize,
        presentation: EventPresentation,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let event = agenda.events.get(event_index)?;
        let option_state = OptionState {
            position: SetPosition::new(event_index, agenda.events.len()),
            selected: self.selected_agenda_event.as_ref() == Some(&event.key),
        };
        Some(render_event_option(event, option_state, presentation, cx))
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.layout =
            CalendarLayout::for_window_width(window.bounds().size.width, window.rem_size());
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id("calendar-view")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_header(cx))
            .children(self.render_agenda_caption(colors))
            .children(self.render_notices())
            .child(self.render_week(cx))
    }
}

/// A ghost button that shows only an icon. The label is its accessible name and its tooltip.
#[derive(Clone, Copy)]
pub(super) struct IconControl {
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) icon: AppIcon,
}

impl IconControl {
    pub(super) fn button(self) -> Button {
        Button::new(self.id)
            .ghost()
            .icon(Icon::new(self.icon))
            .accessibility_label(self.label)
            .tooltip(self.label)
    }
}

fn week_navigation_button<S>(
    control: IconControl,
    week_offset: i64,
    cx: &mut Context<CalendarView<S>>,
) -> impl IntoElement
where
    S: CalendarSession + ?Sized + 'static,
{
    div()
        .flex()
        .flex_shrink_0()
        .debug_selector(move || control.id.to_owned())
        .child(
            control
                .button()
                .on_click(cx.listener(move |view, _, _, cx| view.change_week(week_offset, cx))),
        )
}

pub(super) fn render_empty_agenda(status: CalendarStatus, colors: ColorTokens) -> impl IntoElement {
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

fn render_empty_day(colors: ColorTokens) -> impl IntoElement {
    div()
        .w_full()
        .px_4()
        .py_2()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child("No events")
}

/// Where an event option sits in its listbox and whether it is the selected one.
#[derive(Clone, Copy)]
struct OptionState {
    position: SetPosition,
    selected: bool,
}

/// How an event option looks: a row of the agenda or a bar of a day column.
#[derive(Clone, Copy)]
pub(super) enum EventPresentation {
    Row,
    Bar,
}

/// One event as an option of the week's listbox, for either layout.
///
/// The option keeps its id, selector, name and click handling in both layouts. Only its content
/// and styling follow `presentation`.
fn render_event_option<S>(
    agenda_event: &AgendaEvent<'_>,
    option_state: OptionState,
    presentation: EventPresentation,
    cx: &mut Context<CalendarView<S>>,
) -> AnyElement
where
    S: CalendarSession + ?Sized + 'static,
{
    let OptionState { position, selected } = option_state;
    let event_key = agenda_event.key.clone();
    let date_id = event_key.date.strftime("%Y-%m-%d").to_string();
    let event_id = event_key.event_id.as_str().to_owned();
    let accessible_label = calendar_event_accessible_label(&agenda_event.display, event_key.date);
    let colors = cx.theme().semantic_tokens().colors;
    let primary = cx.theme().primary;
    let content = match presentation {
        EventPresentation::Row => {
            render_event_row_content(&agenda_event.display, &agenda_event.key, colors)
        }
        EventPresentation::Bar => {
            render_event_bar_content(&agenda_event.display, &agenda_event.key, colors, primary)
        }
    };
    let id = format!("calendar-event-option-{date_id}-{event_id}");
    let option = match presentation {
        EventPresentation::Row => list_option(id, position, selected, cx).px_4().py_2(),
        EventPresentation::Bar => filled_list_option(
            id,
            position,
            selected,
            OptionFill {
                rest: primary.opacity(BAR_TINT_OPACITY),
                hover: primary.opacity(BAR_HOVER_TINT_OPACITY),
            },
            cx,
        )
        .p_2()
        .border_1()
        .border_color(if selected {
            primary
        } else {
            primary.opacity(BAR_OUTLINE_OPACITY)
        }),
    }
    .debug_selector(move || format!("calendar-event-row-{date_id}-{event_id}"))
    .aria_label(accessible_label)
    .child(content);
    #[cfg(test)]
    let option = option.test_support();
    option
        .on_click(cx.listener(move |view, _, window, cx| {
            view.select_event_from_option(event_key.clone(), window, cx);
        }))
        .into_any_element()
}

/// The accent tint of an unselected event bar, and its outline and hover strengths.
const BAR_TINT_OPACITY: f32 = 0.08;
const BAR_HOVER_TINT_OPACITY: f32 = 0.16;
const BAR_OUTLINE_OPACITY: f32 = 0.3;

fn event_identity(key: &AgendaEventKey) -> String {
    format!(
        "{}-{}",
        key.date.strftime("%Y-%m-%d"),
        key.event_id.as_str()
    )
}

fn render_event_row_content(
    event: &DisplayEvent<'_>,
    key: &AgendaEventKey,
    colors: ColorTokens,
) -> AnyElement {
    let event_identity = event_identity(key);
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
        details = details.child(render_event_location(location, colors));
    }
    div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap_3()
        .child(
            div()
                .debug_selector(move || time_selector.clone())
                .w(rems(11.))
                .flex_shrink_0()
                .pt_0p5()
                .text_sm()
                .font_medium()
                .child(event.time_label.clone()),
        )
        .child(details)
        .into_any_element()
}

/// The content of an event bar: the time, the subject and the location, each on one line.
fn render_event_bar_content(
    event: &DisplayEvent<'_>,
    key: &AgendaEventKey,
    colors: ColorTokens,
    accent: gpui_kit::Hsla,
) -> AnyElement {
    let event_identity = event_identity(key);
    let details_selector = format!("calendar-event-details-{event_identity}");
    let time_selector = format!("calendar-event-time-{event_identity}");
    v_flex()
        .w_full()
        .min_w_0()
        .gap_0p5()
        .child(
            div()
                .debug_selector(move || time_selector.clone())
                .w_full()
                .truncate()
                .text_xs()
                .font_medium()
                .text_color(accent)
                .child(event.bar_time_label.clone()),
        )
        .child(
            div()
                .debug_selector(move || details_selector.clone())
                .w_full()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_medium()
                .child(event.event.subject().unwrap_or("(No title)").to_owned()),
        )
        .children(
            event
                .event
                .location()
                .map(|location| render_event_location(location, colors)),
        )
        .into_any_element()
}

fn render_event_subject(event: &DisplayEvent<'_>) -> impl IntoElement {
    let subject = event.event.subject().unwrap_or("(No title)");
    div().font_medium().child(subject.to_owned())
}

fn render_event_location(location: &str, colors: ColorTokens) -> Div {
    div()
        .w_full()
        .min_w_0()
        .truncate()
        .text_xs()
        .text_color(colors.muted_foreground)
        .child(location.to_owned())
}
