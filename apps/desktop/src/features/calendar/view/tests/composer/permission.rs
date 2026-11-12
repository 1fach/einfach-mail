use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use einfach_mail_application::{CalendarOperationError, CalendarWorkflow, CalendarWriteReadiness};
use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
use jiff::civil;

use super::super::CalendarView;
use super::super::support::utc_fallback_calendar_week;
use crate::test_support::calendar::{
    TestCache, TestProvider, draw_calendar_view, profile, workflow_account_id,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

#[gpui_kit::test]
async fn denied_calendar_write_emits_permission_review_request(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    provider.fail_next_event_creation(CalendarOperationError::AccessDenied);
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        provider.clone(),
        TestCache::default(),
    ));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let permission_review_requested = Arc::new(AtomicBool::new(false));
    let requested = Arc::clone(&permission_review_requested);
    let mut calendar_handle = None;
    let mut subscription = None;
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
        subscription = Some(cx.subscribe(
            &calendar,
            move |_, _, event: &super::super::CalendarViewEvent, _| {
                if matches!(
                    event,
                    super::super::CalendarViewEvent::ReviewCalendarWritePermission
                ) {
                    requested.store(true, Ordering::SeqCst);
                }
            },
        ));
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");
    let _subscription = subscription;

    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(
                calendar.read(app).status,
                super::super::CalendarStatus::UpToDate
            )
    })
    .await;
    assert!(context.cx.update(|cx| {
        calendar.read(cx).write_readiness == CalendarWriteReadiness::Ready
            && matches!(
                calendar.read(cx).status,
                super::super::CalendarStatus::UpToDate
            )
    }));
    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("calendar offers event creation");
    context.simulate_click(new_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("composer exposes the title input");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_input("Synthetic event");
    let submit = context
        .debug_bounds("event-composer-submit")
        .expect("composer offers create action");
    context.simulate_click(submit.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
    })
    .await;
    let review = context
        .debug_bounds("review-calendar-write-permission")
        .expect("denied write exposes an explicit permission review");
    context.simulate_click(review.center(), Modifiers::default());
    context.run_until_parked();

    assert!(permission_review_requested.load(Ordering::SeqCst));
}
