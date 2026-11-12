use std::{
    ops::Deref,
    sync::{Arc, atomic::Ordering},
};

use einfach_mail_application::{
    CalendarOperationError, CalendarSession, CalendarWorkflow, CalendarWorkflowError,
    CalendarWriteReadiness,
};
use einfach_mail_domain::{CalendarAction, CalendarCapabilities, CalendarOccurrenceTime, EventId};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext, px, size};
use jiff::{Span, civil, tz::TimeZone};
use time::{Date, Month};

use self::support::{system_calendar_week, utc_fallback_calendar_week};
use super::{
    AgendaEventKey, AgendaRow, CalendarStatus, CalendarView, CalendarViewEvent,
    bind_calendar_event_navigation_keys, display_events,
};
use crate::test_support::calendar::{
    TestCache, TestProvider, agenda_events, draw_calendar_view, profile,
    profile_with_calendar_capabilities, read_only_calendar_profile, select_default_timed_event,
    timed_event, timed_event_with_id, workflow_account_id,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

#[cfg(test)]
mod agenda_display;
#[cfg(test)]
mod agenda_navigation;
#[cfg(test)]
mod composer;
#[cfg(test)]
mod support;
#[cfg(test)]
mod synchronization;

#[gpui_kit::test]
fn partial_calendar_capabilities_keep_existing_write_controls_all_or_none(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        TestProvider::default(),
        TestCache::default(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| {
        CalendarView::new_for_week(
            profile_with_calendar_capabilities(CalendarCapabilities::from_actions([
                CalendarAction::CreateEvent,
            ])),
            workflow,
            system_calendar_week(week_start, TimeZone::UTC),
            cx,
        )
    });

    draw_calendar_view(context);
    assert!(!context.cx.update(|app| view.read(app).can_write_events()));
    assert!(context.debug_bounds("calendar-new-event").is_none());
    context.update(|window, app| {
        view.update(app, |view, app| view.open_event_composer(window, app));
    });
    assert!(context.debug_bounds("event-composer-fields").is_none());
}
