use std::sync::Arc;

use einfach_mail_application::{CalendarSession, CalendarWriteReadiness};
use einfach_mail_domain::{AccountProfile, CalendarOccurrence, PendingCalendarEventCreation};
use gpui_kit::{Context, EventEmitter, FocusHandle, ScrollHandle, Subscription, Task};

use self::state::{AgendaEventKey, CalendarStatus};
use super::week::CalendarWeek;

mod actions;
mod agenda;
mod copy;
mod render;
mod state;
mod sync;

pub(crate) use self::actions::bind_calendar_event_navigation_keys;
#[cfg(test)]
use agenda::display_events;
#[cfg(test)]
use state::AgendaRow;

pub(crate) enum CalendarViewEvent {
    ReviewCalendarWritePermission,
}

pub(crate) struct CalendarView<S: CalendarSession + ?Sized = dyn CalendarSession> {
    profile: AccountProfile,
    session: Arc<S>,
    week: CalendarWeek,
    events: Vec<CalendarOccurrence>,
    pending_creation: Option<PendingCalendarEventCreation>,
    event_list_focus: FocusHandle,
    agenda_scroll: ScrollHandle,
    selected_agenda_event: Option<AgendaEventKey>,
    sync_state: Option<einfach_mail_application::CalendarSyncState>,
    write_readiness: CalendarWriteReadiness,
    status: CalendarStatus,
    request_id: u64,
    task: Option<Task<()>>,
    composer_subscription: Option<Subscription>,
}

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    pub(crate) fn new(profile: AccountProfile, session: Arc<S>, cx: &mut Context<Self>) -> Self {
        let week = CalendarWeek::for_system(profile.id());
        Self::new_for_week(profile, session, week, cx)
    }

    fn new_for_week(
        profile: AccountProfile,
        session: Arc<S>,
        week: CalendarWeek,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut calendar_view = Self::loading_for_week(profile, session, week, cx);
        calendar_view.refresh(cx);
        calendar_view
    }

    fn loading_for_week(
        profile: AccountProfile,
        session: Arc<S>,
        week: CalendarWeek,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            profile,
            session,
            week,
            events: Vec::new(),
            pending_creation: None,
            event_list_focus: cx.focus_handle().tab_stop(true),
            agenda_scroll: ScrollHandle::new(),
            selected_agenda_event: None,
            sync_state: None,
            write_readiness: CalendarWriteReadiness::ViewSyncRequired,
            status: CalendarStatus::Idle,
            request_id: 0,
            task: None,
            composer_subscription: None,
        }
    }
}

impl<S> EventEmitter<CalendarViewEvent> for CalendarView<S> where
    S: CalendarSession + ?Sized + 'static
{
}

#[cfg(test)]
mod tests;
