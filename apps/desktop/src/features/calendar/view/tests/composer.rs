use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use einfach_mail_application::{CalendarOperationError, CalendarWorkflow, CalendarWriteReadiness};
use einfach_mail_domain::{
    CalendarAction, CalendarCapabilities, CalendarEventCreation, CalendarEventDraft,
    CalendarTimeZoneId, CalendarViewReference, PendingCalendarEventCreation,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{
    AppContext as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Role, TestAppContext,
    VisualTestContext,
};
use jiff::civil;
use time::{Date, Month, PrimitiveDateTime, Time};
use tokio::sync::oneshot;

use super::CalendarView;
use super::support::{system_calendar_view, utc_fallback_calendar_week};
use crate::test_support::calendar::{
    TestCache, TestProvider, draw_calendar_view, profile, profile_with_calendar_capabilities,
    read_only_calendar_profile, select_default_timed_event, workflow_account_id,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

fn saved_pending_creation(view: CalendarViewReference) -> CalendarEventCreation {
    let date =
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic event date is valid");
    let local_start = PrimitiveDateTime::new(
        date,
        Time::from_hms(9, 30, 0).expect("synthetic start time is valid"),
    );
    let local_end = PrimitiveDateTime::new(
        date,
        Time::from_hms(10, 0, 0).expect("synthetic end time is valid"),
    );
    let time_zone =
        CalendarTimeZoneId::try_from("Etc/UTC".to_owned()).expect("synthetic time zone is valid");
    let draft = CalendarEventDraft::timed(
        "Pending EWS appointment".to_owned(),
        local_start,
        local_end,
        time_zone,
        local_start.assume_utc(),
        local_end.assume_utc(),
    )
    .expect("synthetic event draft is valid");
    CalendarEventCreation::try_new(view, draft)
        .expect("synthetic event endpoints fit the selected week")
}

fn press_enter(context: &mut VisualTestContext) {
    let keystroke = Keystroke::parse("enter").expect("Enter keystroke is valid");
    context.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    context.simulate_event(KeyUpEvent { keystroke });
    context.run_until_parked();
}

#[gpui_kit::test]
async fn native_event_composer_renders_date_pickers_and_recovers_create_outcomes(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let cache = TestCache::default();
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        provider.clone(),
        cache.clone(),
    ));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let mut calendar_handle = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let calendar = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                Arc::clone(&workflow),
                utc_fallback_calendar_week(week_start),
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "calendar-sync-error")
    })
    .await;
    assert!(
        context.debug_bounds("calendar-sync-error").is_some(),
        "calendar sync reaches a visible error state"
    );

    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("calendar offers event creation");
    context.simulate_click(new_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(!context.update(|window, cx| window.has_active_sheet(cx)));
    provider.allow_calendar_sync();
    let sync = context
        .debug_bounds("calendar-sync")
        .expect("calendar offers a retry after the initial sync fails");
    context.simulate_click(sync.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(context.cx.update(|cx| {
        calendar.read(cx).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate)
    }));
    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("synchronized calendar offers event creation");
    context.simulate_click(new_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.update(|window, cx| window.has_active_sheet(cx)));
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    assert!(context.debug_bounds("event-composer-fields").is_some());
    let date_picker_count =
        context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ComboBox).len());
    assert_eq!(date_picker_count, 2, "both date pickers remain available");
    let create_event = context
        .debug_bounds("event-composer-submit")
        .expect("composer offers create action");
    context.simulate_click(create_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-composer-validation").is_some());
    assert!(context.debug_bounds("event-creation-pending").is_none());

    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("composer exposes the title input");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_input("Synthetic event");
    let (release, gate) = oneshot::channel();
    provider.hold_event_creation_until(gate);
    provider.fail_next_event_creation(CalendarOperationError::ServiceUnavailable);
    let create_event = context
        .debug_bounds("event-composer-submit")
        .expect("composer can submit the completed draft");
    context.simulate_click(create_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-creation-submitting").is_some());
    release
        .send(())
        .expect("synthetic provider request is waiting on this test");

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
    })
    .await;
    assert!(context.debug_bounds("event-creation-pending").is_some());
    assert!(context.debug_bounds("event-creation-status").is_some());

    let status_check = context
        .debug_bounds("event-composer-submit")
        .expect("the saved operation offers a status check");
    assert_eq!(
        provider.event_creation_count.load(Ordering::SeqCst),
        1,
        "the uncertain creation was sent once"
    );
    context.simulate_click(status_check.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
    })
    .await;
    assert!(context.debug_bounds("event-creation-pending").is_none());
    assert!(context.debug_bounds("event-creation-status").is_some());
    assert!(context.debug_bounds("event-composer-close").is_some());
    assert_eq!(
        provider.event_creation_count.load(Ordering::SeqCst),
        1,
        "checking the saved operation never resends its creation"
    );

    let close = context
        .debug_bounds("event-composer-close")
        .expect("confirmed composer can be closed");
    context.simulate_click(close.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);

    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("calendar offers another event creation");
    context.simulate_click(new_event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-composer-fields").is_some());
    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("composer exposes the title input");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_input("Rejected synthetic event");
    provider.fail_next_event_creation(CalendarOperationError::EventCreationRejected);
    cache.fail_rejected_cleanup_once();

    let create_event = context
        .debug_bounds("event-composer-submit")
        .expect("composer offers create action");
    context.simulate_click(create_event.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
    })
    .await;
    assert!(context.debug_bounds("event-creation-pending").is_some());
    assert!(context.debug_bounds("event-creation-status").is_some());
    let rejected_marker = workflow
        .load_cached_view(&system_calendar_view(week_start))
        .await
        .expect("rejected operation remains available for cleanup");
    assert!(matches!(
        rejected_marker.pending_creation(),
        Some(PendingCalendarEventCreation::Rejected(_))
    ));
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 2);

    let clear_rejected = context
        .debug_bounds("event-composer-submit")
        .expect("rejected operation offers local cleanup");
    context.simulate_click(clear_rejected.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
            && !test_element_is_visible(window, "event-creation-pending")
    })
    .await;
    assert!(context.debug_bounds("event-creation-pending").is_none());
    assert!(context.debug_bounds("event-composer-close").is_some());
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 2);
    let cleared_marker = workflow
        .load_cached_view(&system_calendar_view(week_start))
        .await
        .expect("calendar view can read its cache");
    assert!(cleared_marker.pending_creation().is_none());
}

#[gpui_kit::test]
async fn read_only_calendar_checks_pending_creation_status_by_keyboard(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    let cache = TestCache::default();
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let view = system_calendar_view(week_start);
    let creation = saved_pending_creation(view.clone());

    let initial_workflow =
        CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
    initial_workflow
        .sync_view(&view)
        .await
        .expect("the initial synthetic calendar sync succeeds");
    provider.fail_next_event_creation(CalendarOperationError::ServiceUnavailable);
    assert_eq!(
        initial_workflow.create_event(&creation).await,
        Err(einfach_mail_application::CalendarWorkflowError::Provider(
            CalendarOperationError::ServiceUnavailable
        ))
    );
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 1);
    drop(initial_workflow);

    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        provider.clone(),
        cache,
    ));
    let mut calendar_handle = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let calendar = cx.new(|cx| {
            CalendarView::new_for_week(
                read_only_calendar_profile(),
                Arc::clone(&workflow),
                utc_fallback_calendar_week(week_start),
                cx,
            )
        });
        calendar_handle = Some(calendar.clone());
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");
    wait_for_ui_condition(context, |window, app| {
        test_element_is_visible(window, "calendar-new-event")
            && calendar.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(context.debug_bounds("calendar-read-only-status").is_some());
    let toolbar_labels = context.update(|window, _| {
        window
            .find_all_by_role(Role::Button)
            .iter()
            .filter_map(|button| button.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(
        toolbar_labels
            .iter()
            .any(|label| label == "Check event status…")
    );

    let check_status = context
        .debug_bounds("calendar-new-event")
        .expect("read-only calendar exposes the saved event status check");
    context.simulate_click(check_status.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-pending")
    })
    .await;
    assert!(context.debug_bounds("event-composer-fields").is_none());
    let status_labels = context.update(|window, _| {
        window
            .find_all_by_role(Role::Status)
            .iter()
            .filter_map(|status| status.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    let saved_draft_label = status_labels
        .iter()
        .find(|label| label.starts_with("Saved event draft:"))
        .expect("the saved draft has an accessible identity");
    assert!(
        saved_draft_label.contains("Pending EWS appointment"),
        "the exact saved draft remains visible: {saved_draft_label}"
    );

    let (release, gate) = oneshot::channel();
    provider.hold_event_creation_recovery_until(gate);
    provider.fail_next_event_creation_recovery(CalendarOperationError::CreationOutcomeUnknown);
    context.simulate_keystrokes("tab");
    let mut status_focused = context
        .update(|window, _| window.find_by_label("Check event status").focused() == Some(true));
    for _ in 0..6 {
        if status_focused {
            break;
        }
        context.simulate_keystrokes("tab");
        status_focused = context
            .update(|window, _| window.find_by_label("Check event status").focused() == Some(true));
    }
    assert!(status_focused, "Tab reaches the status-check action");
    press_enter(context);
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(
        context.debug_bounds("event-creation-checking").is_some(),
        "the pending check stays visible while provider recovery is gated; recovery count: {}",
        provider
            .event_creation_recovery_count
            .load(Ordering::SeqCst)
    );
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 1);
    release
        .send(())
        .expect("synthetic recovery is waiting on this test");

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
            && test_element_is_visible(window, "event-creation-pending")
    })
    .await;
    let unresolved_labels = context.update(|window, _| {
        window
            .find_all_by_role(Role::Alert)
            .iter()
            .filter_map(|status| status.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(
        unresolved_labels
            .iter()
            .any(|label| label.contains("could not confirm whether this event exists")),
        "the inconclusive result stays unresolved: {unresolved_labels:?}"
    );
    assert!(context.update(|window, _| {
        window
            .find_all_by_role(Role::Status)
            .iter()
            .any(|status| status.label() == Some(saved_draft_label.as_str()))
    }));
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 1);

    let (release_failed_check, failed_check_gate) = oneshot::channel();
    provider.hold_event_creation_recovery_until(failed_check_gate);
    provider.fail_next_event_creation_recovery(CalendarOperationError::ServiceUnavailable);
    let check_status = context
        .debug_bounds("event-composer-submit")
        .expect("an unresolved event can be checked again");
    context.simulate_click(check_status.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-creation-checking").is_some());
    release_failed_check
        .send(())
        .expect("the synthetic failed status request is waiting");
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-creation-status")
            && test_element_is_visible(window, "event-creation-pending")
            && !test_element_is_visible(window, "event-creation-checking")
    })
    .await;
    assert!(context.debug_bounds("event-composer-submit").is_some());
    let failed_labels = context.update(|window, _| {
        window
            .find_all_by_role(Role::Alert)
            .iter()
            .filter_map(|status| status.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(
        failed_labels
            .iter()
            .any(|label| label.contains("calendar service is unavailable")),
        "the failed status request is visible: {failed_labels:?}"
    );
    assert!(context.update(|window, _| {
        window
            .find_all_by_role(Role::Status)
            .iter()
            .any(|status| status.label() == Some(saved_draft_label.as_str()))
    }));
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 1);

    let check_status = context
        .debug_bounds("event-composer-submit")
        .expect("the failed check leaves the saved status action available");
    context.simulate_click(check_status.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-close")
            && !test_element_is_visible(window, "event-creation-pending")
    })
    .await;
    assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider
            .event_creation_recovery_count
            .load(Ordering::SeqCst),
        3
    );
    let snapshot = workflow
        .load_cached_view(&view)
        .await
        .expect("reconciled event is readable after status recovery");
    assert!(snapshot.pending_creation().is_none());
    assert_eq!(
        snapshot
            .events()
            .iter()
            .filter(|event| event.subject() == Some("Pending EWS appointment"))
            .count(),
        1,
        "the exact saved event is reconciled once"
    );
}

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
            move |_, _, event: &super::CalendarViewEvent, _| {
                if matches!(
                    event,
                    super::CalendarViewEvent::ReviewCalendarWritePermission
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
            && matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(context.cx.update(|cx| {
        calendar.read(cx).write_readiness == CalendarWriteReadiness::Ready
            && matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate)
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

#[gpui_kit::test]
async fn native_event_editor_updates_a_cached_timed_appointment(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(super::super::super::composer::bind_event_composer_keys);
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
        matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| { matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate) })
    );

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

#[gpui_kit::test]
async fn native_event_editor_confirms_and_serializes_deletion(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(super::super::super::composer::bind_event_composer_keys);
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
        gpui_kit::base::Root::new(calendar, window, cx)
    });
    let calendar = calendar_handle.expect("calendar entity is retained for assertions");

    wait_for_ui_condition(context, |_, app| {
        matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| { matches!(calendar.read(cx).status, super::CalendarStatus::UpToDate) })
    );

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
            && matches!(calendar.read(app).status, super::CalendarStatus::UpToDate)
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
