use std::sync::{Arc, atomic::Ordering};

use einfach_mail_application::{CalendarOperationError, CalendarWorkflow, CalendarWriteReadiness};
use gpui_kit::component::WindowExt as _;
use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
use jiff::civil;
use tokio::sync::oneshot;

use super::super::CalendarView;
use super::super::support::utc_fallback_calendar_week;
use crate::test_support::calendar::{
    TestCache, TestProvider, draw_calendar_view, profile, select_default_timed_event,
    workflow_account_id,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

#[gpui_kit::test]
async fn native_event_editor_confirms_and_serializes_deletion(cx: &mut TestAppContext) {
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
                profile(),
                workflow,
                utc_fallback_calendar_week(week_start),
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        super::super::support::calendar_workspace(calendar, window, cx)
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
        .expect("timed appointment offers an edit action");
    context.simulate_click(edit.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-actions")
    })
    .await;
    assert!(context.debug_bounds("event-composer-delete").is_some());

    let (release, gate) = oneshot::channel();
    provider.hold_event_deletion_until(gate);
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("editor offers a delete action");
    context.simulate_click(delete.center(), Modifiers::default());
    let cancel = context
        .debug_bounds("event-delete-dialog-cancel")
        .expect("delete opens a confirmation dialog");
    assert!(
        context
            .debug_bounds("event-delete-dialog-confirm")
            .is_some()
    );
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        0,
        "opening confirmation does not start deletion"
    );
    context.simulate_click(cancel.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        0,
        "canceling confirmation leaves the event untouched"
    );

    provider.fail_next_event_deletion(CalendarOperationError::DeleteOutcomeUnknown);
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("cancel returns to the event editor");
    context.simulate_click(delete.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, app| window.has_active_dialog(app)).await;
    let confirm = context
        .debug_bounds("event-delete-dialog-confirm")
        .expect("confirmation offers the destructive action");
    context.simulate_click(confirm.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider.event_deletion_calls.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        1,
        "confirmation starts one delete request"
    );
    context.simulate_keystrokes("escape");
    draw_calendar_view(context);
    assert!(
        context.update(|window, cx| window.has_active_sheet(cx)),
        "Escape cannot hide a pending delete"
    );
    let event_list = context
        .debug_bounds("calendar-event-list")
        .expect("calendar remains behind the editor");
    context.simulate_click(
        gpui_kit::point(event_list.left() + gpui_kit::px(1.), event_list.center().y),
        Modifiers::default(),
    );
    draw_calendar_view(context);
    assert!(
        context.update(|window, cx| window.has_active_sheet(cx)),
        "an outside click cannot hide a pending delete"
    );
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("pending deletion keeps its disabled action visible");
    context.simulate_click(delete.center(), Modifiers::default());
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        1,
        "a pending delete cannot be submitted twice"
    );

    release
        .send(())
        .expect("synthetic provider is waiting on the UI test");
    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness != CalendarWriteReadiness::Ready
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| calendar.read(cx).write_readiness != CalendarWriteReadiness::Ready)
    );
    let done = context
        .debug_bounds("event-composer-close")
        .expect("uncertain delete can be dismissed");
    context.simulate_click(done.center(), Modifiers::default());
    draw_calendar_view(context);

    let sync = context
        .debug_bounds("calendar-sync")
        .expect("calendar offers recovery sync");
    context.simulate_click(sync.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(
                calendar.read(app).status,
                super::super::CalendarStatus::UpToDate
            )
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| { calendar.read(cx).write_readiness == CalendarWriteReadiness::Ready })
    );

    let edit = context
        .debug_bounds("calendar-event-open")
        .expect("successful sync restores event editing");
    context.simulate_click(edit.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-actions")
    })
    .await;
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("reconciled event can be deleted again");
    context.simulate_click(delete.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, app| window.has_active_dialog(app)).await;
    let confirm = context
        .debug_bounds("event-delete-dialog-confirm")
        .expect("reconciled event offers delete confirmation");
    context.simulate_click(confirm.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider.event_deletion_calls.load(Ordering::SeqCst) == 2
    })
    .await;
    assert_eq!(
        provider.event_deletion_calls.load(Ordering::SeqCst),
        2,
        "a successful sync permits a new explicit delete"
    );
    wait_for_ui_condition(context, |_, app| {
        calendar
            .read(app)
            .events
            .iter()
            .all(|event| event.id().as_str() != "synthetic-timed-event")
    })
    .await;
    let deleted = context.cx.update(|cx| {
        calendar
            .read(cx)
            .events
            .iter()
            .all(|event| event.id().as_str() != "synthetic-timed-event")
    });
    assert!(deleted, "confirmed delete removes the cached occurrence");
    assert!(context.debug_bounds("event-creation-status").is_some());
    assert!(context.debug_bounds("event-composer-close").is_some());
}
