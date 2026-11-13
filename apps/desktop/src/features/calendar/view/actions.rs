use std::sync::Arc;

use einfach_mail_application::{CalendarSession, CalendarWriteReadiness};
use einfach_mail_domain::{CalendarAction, CalendarOccurrence, CalendarOccurrenceTime};
use gpui_kit::component::{StyledExt, WindowExt};
use gpui_kit::{App, Context, Entity, KeyBinding, Window, div, prelude::*};
use jiff::{Span, civil};

use crate::components::ListNavigation;

use super::super::composer::{
    EventComposer, EventComposerEvent, EventComposerIntent, EventComposerSetup,
};
use super::super::week::monday_start;
use super::copy::event_creation_action_copy;
use super::state::{AgendaEventKey, CalendarStatus};
use super::{CalendarView, CalendarViewEvent};

gpui_kit::actions!(
    calendar_event_list,
    [
        SelectPreviousCalendarEvent,
        SelectNextCalendarEvent,
        SelectFirstCalendarEvent,
        SelectLastCalendarEvent,
        OpenSelectedCalendarEvent,
    ]
);

pub(super) const CALENDAR_EVENT_LIST_CONTEXT: &str = "CalendarEventList";

#[derive(Clone, Copy)]
pub(super) enum SelectedEventAction {
    Edit,
    Details,
}

impl SelectedEventAction {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Details => "Details",
        }
    }

    const fn sheet_title(self) -> &'static str {
        match self {
            Self::Edit => "Edit calendar event",
            Self::Details => "Calendar event details",
        }
    }
}

pub(crate) fn bind_calendar_event_navigation_keys(cx: &mut App) {
    let context = Some(CALENDAR_EVENT_LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPreviousCalendarEvent, context),
        KeyBinding::new("down", SelectNextCalendarEvent, context),
        KeyBinding::new("home", SelectFirstCalendarEvent, context),
        KeyBinding::new("end", SelectLastCalendarEvent, context),
        KeyBinding::new("enter", OpenSelectedCalendarEvent, context),
    ]);
    super::navigation::bind_mini_month_keys(cx);
}

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    pub(super) fn open_event_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if (self.pending_creation.is_none()
            && !self.supports_calendar_action(CalendarAction::CreateEvent))
            || self.write_readiness != CalendarWriteReadiness::Ready
        {
            return;
        }
        let intent = EventComposerIntent::Create {
            pending_creation: self.pending_creation.clone(),
        };
        let action_copy = event_creation_action_copy(self.pending_creation.as_ref());
        self.open_composer(intent, action_copy.sheet_title, window, cx);
    }

    pub(super) fn open_selected_event(
        &mut self,
        cached_event: CalendarOccurrence,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = self.selected_event_action() else {
            return;
        };
        if self.write_readiness != CalendarWriteReadiness::Ready
            || !matches!(cached_event.time(), CalendarOccurrenceTime::Timed { .. })
        {
            return;
        }
        self.open_composer(
            EventComposerIntent::Existing { cached_event },
            action.sheet_title(),
            window,
            cx,
        );
    }

    pub(super) fn selected_event_action(&self) -> Option<SelectedEventAction> {
        if self.supports_calendar_action(CalendarAction::UpdateEvent) {
            Some(SelectedEventAction::Edit)
        } else if self.supports_calendar_action(CalendarAction::DeleteEvent) {
            Some(SelectedEventAction::Details)
        } else {
            None
        }
    }

    pub(super) fn selected_calendar_event(&self) -> Option<CalendarOccurrence> {
        let selected = self.selected_agenda_event.as_ref()?;
        self.events
            .iter()
            .find(|event| event.id() == &selected.event_id)
            .cloned()
    }

    fn open_composer(
        &mut self,
        intent: EventComposerIntent,
        title: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.status.is_sync_active() || window.has_active_sheet(cx) {
            return;
        }
        let setup = self.event_composer_setup(intent);
        let composer = cx.new(|cx| EventComposer::new(setup, window, cx));
        self.composer_subscription = Some(cx.subscribe(
            &composer,
            |view, _, event: &EventComposerEvent, cx| match event {
                EventComposerEvent::CalendarChanged
                | EventComposerEvent::PendingOperationUpdated => view.reload_cached_view(cx),
                EventComposerEvent::CalendarWriteSyncRequired => {
                    view.require_calendar_sync_for_write(cx);
                }
                EventComposerEvent::ReviewCalendarWritePermission => {
                    cx.emit(CalendarViewEvent::ReviewCalendarWritePermission);
                }
            },
        ));
        Self::open_event_composer_sheet(title, composer, window, cx);
    }

    pub(super) fn event_composer_setup(
        &self,
        intent: EventComposerIntent,
    ) -> EventComposerSetup<S> {
        EventComposerSetup {
            session: Arc::clone(&self.session),
            week: self.week.clone(),
            capabilities: self.profile.capabilities().calendar(),
            intent,
        }
    }

    fn open_event_composer_sheet(
        title: &'static str,
        composer: Entity<EventComposer<S>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.open_sheet(cx, move |sheet, _, cx| {
            sheet
                .overlay_closable(!composer.read(cx).edit_write_in_progress())
                .title(div().text_base().font_bold().child(title))
                .child(composer.clone())
        });
    }

    fn navigate_event(&mut self, navigation: ListNavigation, cx: &mut Context<Self>) {
        let agenda = self.week_agenda();
        let selected_index = self
            .selected_agenda_event
            .as_ref()
            .and_then(|selected| agenda.position_of(selected));
        let Some(target_index) = navigation.target_index(selected_index, agenda.events.len())
        else {
            return;
        };
        let Some(target) = agenda.events.get(target_index) else {
            return;
        };
        let selected_event = target.key.clone();
        let scroll_index = agenda.scroll_index(target_index, self.layout);
        self.agenda_scroll.scroll_to_item(scroll_index);
        self.selected_agenda_event = Some(selected_event);
        cx.notify();
    }

    pub(super) fn select_event_from_option(
        &mut self,
        selected_event: AgendaEventKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agenda = self.week_agenda();
        let Some(event_index) = agenda.position_of(&selected_event) else {
            return;
        };
        let scroll_index = agenda.scroll_index(event_index, self.layout);
        self.agenda_scroll.scroll_to_item(scroll_index);
        self.event_list_focus.focus(window, cx);
        self.selected_agenda_event = Some(selected_event);
        cx.notify();
    }

    pub(super) fn on_select_previous_calendar_event(
        &mut self,
        _: &SelectPreviousCalendarEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_event(ListNavigation::Previous, cx);
    }

    pub(super) fn on_select_next_calendar_event(
        &mut self,
        _: &SelectNextCalendarEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_event(ListNavigation::Next, cx);
    }

    pub(super) fn on_select_first_calendar_event(
        &mut self,
        _: &SelectFirstCalendarEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_event(ListNavigation::First, cx);
    }

    pub(super) fn on_select_last_calendar_event(
        &mut self,
        _: &SelectLastCalendarEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_event(ListNavigation::Last, cx);
    }

    pub(super) fn on_open_selected_calendar_event(
        &mut self,
        _: &OpenSelectedCalendarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(event) = self.selected_calendar_event() else {
            return;
        };
        self.open_selected_event(event, window, cx);
    }

    pub(super) fn change_week(&mut self, weeks: i64, cx: &mut Context<Self>) {
        let Some(week_start) = self.week.start().checked_add(Span::new().weeks(weeks)).ok() else {
            return;
        };
        self.show_week_starting(week_start, cx);
    }

    /// Shows the week that contains today.
    pub(super) fn show_today(&mut self, cx: &mut Context<Self>) {
        self.show_date(self.clock.today(), cx);
    }

    /// Shows the week that contains `date`.
    pub(super) fn show_date(&mut self, date: civil::Date, cx: &mut Context<Self>) {
        if let Some(week_start) = monday_start(date) {
            self.show_week_starting(week_start, cx);
        }
    }

    /// Whether the shown week is the one that contains today.
    pub(super) fn shows_today(&self) -> bool {
        monday_start(self.clock.today()) == Some(self.week.start())
    }

    fn show_week_starting(&mut self, week_start: civil::Date, cx: &mut Context<Self>) {
        if week_start == self.week.start() {
            return;
        }
        let Some(week) = self.week.with_start(week_start) else {
            return;
        };
        drop(self.task.take());
        self.request_id = self.request_id.saturating_add(1);
        self.status = CalendarStatus::Idle;
        self.week = week;
        self.events.clear();
        self.write_readiness = CalendarWriteReadiness::ViewSyncRequired;
        self.agenda_scroll.scroll_to_item(0);
        self.selected_agenda_event = None;
        self.sync_state = None;
        self.refresh(cx);
    }

    pub(super) fn supports_calendar_action(&self, action: CalendarAction) -> bool {
        self.profile.capabilities().calendar().supports(action)
    }
}
