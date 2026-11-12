use std::sync::{Arc, atomic::Ordering};

use einfach_mail_application::CalendarWorkflow;
use einfach_mail_domain::{CalendarAction, CalendarCapabilities};
use gpui_kit::component::WindowExt as _;
use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
use jiff::civil;
use tokio::sync::oneshot;

use super::super::CalendarView;
use super::super::support::utc_fallback_calendar_week;
use crate::test_support::calendar::{
    TestCache, TestProvider, draw_calendar_view, profile_with_calendar_capabilities,
    select_default_timed_event, workflow_account_id,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

#[gpui_kit::test]
async fn native_event_editor_updates_a_cached_timed_appointment(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::features::calendar::composer::bind_event_composer_keys);
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        provider.clone(),
        TestCache::default(),
    ));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let mut calendar_handle = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let calendar = cx.new(|cx| {
            CalendarView::new_for_week(
                profile_with_calendar_capabilities(CalendarCapabilities::from_actions([
                    CalendarAction::UpdateEvent,
                ])),
                workflow,
                utc_fallback_calendar_week(week_start),
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");

    wait_for_ui_condition(context, |_, app| {
        matches!(
            calendar.read(app).status,
            super::super::CalendarStatus::UpToDate
        )
    })
    .await;
    assert!(context.cx.update(|cx| {
        matches!(
            calendar.read(cx).status,
            super::super::CalendarStatus::UpToDate
        )
    }));

    select_default_timed_event(context);
    let edit = context
        .debug_bounds("calendar-event-open")
        .expect("timed calendar event offers an edit action");
    context.simulate_click(edit.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    assert!(context.debug_bounds("event-composer-fields").is_some());
    assert!(context.debug_bounds("event-composer-delete").is_none());

    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("editor exposes the event title");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_keystrokes("ctrl-a");
    context.simulate_input("Updated synthetic appointment");
    draw_calendar_view(context);

    let save = context
        .debug_bounds("event-composer-submit")
        .expect("editor offers a save action");
    let (release, gate) = oneshot::channel();
    provider.hold_event_update_until(gate);
    context.simulate_click(save.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider.event_update_calls.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        provider.event_update_calls.load(Ordering::SeqCst),
        1,
        "saving starts one update request"
    );
    let event_list = context
        .debug_bounds("calendar-event-list")
        .expect("the calendar remains behind the appointment editor");
    context.simulate_click(
        gpui_kit::point(event_list.left() + gpui_kit::px(1.), event_list.center().y),
        Modifiers::default(),
    );
    draw_calendar_view(context);
    assert!(
        context.update(|window, cx| window.has_active_sheet(cx)),
        "an outside click cannot hide a pending update"
    );
    context.simulate_keystrokes("escape");
    draw_calendar_view(context);
    assert!(
        context.update(|window, cx| window.has_active_sheet(cx)),
        "Escape cannot hide a pending update"
    );
    let saving = context
        .debug_bounds("event-composer-submit")
        .expect("pending update keeps its disabled Save action visible");
    context.simulate_click(saving.center(), Modifiers::default());
    assert_eq!(
        provider.event_update_calls.load(Ordering::SeqCst),
        1,
        "a pending update cannot be submitted twice"
    );
    release
        .send(())
        .expect("synthetic provider is waiting on the UI test");
    wait_for_ui_condition(context, |_, app| {
        calendar
            .read(app)
            .events
            .iter()
            .any(|event| event.subject() == Some("Updated synthetic appointment"))
    })
    .await;
    let (updated, saved_status) = context.cx.update(|cx| {
        let view = calendar.read(cx);
        (
            view.events
                .iter()
                .any(|event| event.subject() == Some("Updated synthetic appointment")),
            view.events.len(),
        )
    });
    assert!(
        updated,
        "confirmed edit reloads the updated cached occurrence"
    );
    assert_eq!(
        saved_status, 2,
        "the calendar retains both synthetic events"
    );
    assert!(context.debug_bounds("event-creation-status").is_some());
    assert!(context.debug_bounds("event-composer-close").is_some());
}
