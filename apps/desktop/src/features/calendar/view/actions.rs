use std::sync::Arc;

use einfach_mail_application::{CalendarSession, CalendarWriteReadiness};
use einfach_mail_domain::{CalendarAction, CalendarOccurrence, CalendarOccurrenceTime};
use gpui_kit::component::{StyledExt, WindowExt};
use gpui_kit::{App, Context, Entity, KeyBinding, Window, div, prelude::*};
use jiff::Span;

use crate::components::ListNavigation;

use super::super::composer::{
    EventComposer, EventComposerEvent, EventComposerIntent, EventComposerSetup,
};
use super::state::{AgendaEventKey, AgendaRow, CalendarStatus};
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

pub(crate) fn bind_calendar_event_navigation_keys(cx: &mut App) {
    let context = Some(CALENDAR_EVENT_LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPreviousCalendarEvent, context),
        KeyBinding::new("down", SelectNextCalendarEvent, context),
        KeyBinding::new("home", SelectFirstCalendarEvent, context),
        KeyBinding::new("end", SelectLastCalendarEvent, context),
        KeyBinding::new("enter", OpenSelectedCalendarEvent, context),
    ]);
}

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    pub(super) fn open_event_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_write_events() || self.write_readiness != CalendarWriteReadiness::Ready {
            return;
        }
        let intent = EventComposerIntent::Create {
            pending_creation: self.pending_creation.clone(),
        };
        let title = if self.pending_creation.is_some() {
            "Resolve pending event"
        } else {
            "New calendar event"
        };
        self.open_composer(intent, title, window, cx);
    }

    pub(super) fn open_event_editor(
        &mut self,
        cached_event: CalendarOccurrence,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_write_events()
            || self.write_readiness != CalendarWriteReadiness::Ready
            || !matches!(cached_event.time(), CalendarOccurrenceTime::Timed { .. })
        {
            return;
        }
        self.open_composer(
            EventComposerIntent::Edit { cached_event },
            "Edit calendar event",
            window,
            cx,
        );
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
        let rows = self.agenda_rows();
        let event_rows = rows
            .iter()
            .enumerate()
            .filter_map(|(row_index, row)| match row {
                AgendaRow::Event(event) => Some((row_index, &event.key)),
                AgendaRow::DayHeading(_) | AgendaRow::EmptyDay => None,
            })
            .collect::<Vec<_>>();
        let selected_index = self
            .selected_agenda_event
            .as_ref()
            .and_then(|selected| event_rows.iter().position(|(_, key)| *key == selected));
        let Some(target_index) = navigation.target_index(selected_index, event_rows.len()) else {
            return;
        };
        let Some((scroll_index, selected_event)) = event_rows.get(target_index) else {
            return;
        };
        let scroll_index = *scroll_index;
        let selected_event = (*selected_event).clone();
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
        let rows = self.agenda_rows();
        let Some(scroll_index) = rows
            .iter()
            .position(|row| matches!(row, AgendaRow::Event(event) if event.key == selected_event))
        else {
            return;
        };
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
        let Some(selected) = self.selected_agenda_event.as_ref() else {
            return;
        };
        let Some(event) = self
            .events
            .iter()
            .find(|event| event.id() == &selected.event_id)
            .cloned()
        else {
            return;
        };
        self.open_event_editor(event, window, cx);
    }

    pub(super) fn change_week(&mut self, weeks: i64, cx: &mut Context<Self>) {
        let Some(week_start) = self.week.start().checked_add(Span::new().weeks(weeks)).ok() else {
            return;
        };
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

    pub(super) fn can_write_events(&self) -> bool {
        let calendar = self.profile.capabilities().calendar();
        [
            CalendarAction::CreateEvent,
            CalendarAction::UpdateEvent,
            CalendarAction::DeleteEvent,
        ]
        .into_iter()
        .all(|action| calendar.supports(action))
    }
}
