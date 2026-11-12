use super::*;

#[test]
fn sync_activity_matches_calendar_status() {
    assert!(CalendarStatus::LoadingCache.is_sync_active());
    assert!(CalendarStatus::Syncing.is_sync_active());
    assert!(!CalendarStatus::Idle.is_sync_active());
    assert!(!CalendarStatus::UpToDate.is_sync_active());
    assert!(!CalendarStatus::Failed(CalendarWorkflowError::AccountMismatch).is_sync_active());
}

#[gpui_kit::test]
async fn native_calendar_view_keeps_cached_events_and_navigates_fixed_weeks(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let provider = TestProvider::default();
    let requests = Arc::clone(&provider.requests);
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        provider,
        TestCache::default(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| {
        CalendarView::new_for_week(
            profile(),
            workflow,
            utc_fallback_calendar_week(week_start),
            cx,
        )
    });

    context.run_until_parked();
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "calendar-sync-error")
    })
    .await;
    let (sync_in_progress, status_failed) = context.cx.update(|cx| {
        let view = view.read(cx);
        (
            view.status.is_sync_active(),
            matches!(view.status, super::CalendarStatus::Failed(_)),
        )
    });
    assert!(
        !sync_in_progress && status_failed,
        "calendar refresh completes"
    );
    assert!(context.debug_bounds("calendar-sync-error").is_some());
    assert!(context.debug_bounds("calendar-utc-fallback").is_some());
    let status_labels = context.update(|window, _| {
        window
            .find_all_by_role(gpui_kit::Role::Status)
            .iter()
            .map(|status| status.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(
        status_labels.iter().any(|label| {
            label.as_deref()
                == Some("The system time zone is unavailable. Timed events are shown in UTC.")
        }),
        "the UTC fallback status label is accessible; found {status_labels:?}"
    );
    assert!(
        context
            .debug_bounds("calendar-event-row-2030-01-07-synthetic-ui-event")
            .is_some()
    );
    assert!(context.debug_bounds("calendar-next-week").is_some());

    let next_week_button = context
        .debug_bounds("calendar-next-week")
        .expect("week navigation button is rendered");
    context.simulate_click(next_week_button.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);

    let (selected_week, range, utc_fallback) = context.cx.update(|cx| {
        let view = view.read(cx);
        (
            view.week.start(),
            view.week.view().range().clone(),
            view.week.is_utc_fallback(),
        )
    });
    assert_eq!(
        selected_week,
        week_start
            .checked_add(Span::new().weeks(1))
            .expect("following week is in range")
    );
    assert_eq!(
        range.start_date(),
        Date::from_calendar_date(2030, Month::January, 14).expect("valid date")
    );
    assert!(
        utc_fallback,
        "navigation keeps the selected time-zone choice"
    );
    let composer_week = context.cx.update(|cx| {
        view.read(cx)
            .event_composer_setup(super::super::super::composer::EventComposerIntent::Create {
                pending_creation: None,
            })
            .week
    });
    assert_eq!(composer_week.start(), selected_week);
    assert_eq!(
        composer_week.view().range().start_date(),
        range.start_date()
    );
    assert_eq!(composer_week.is_utc_fallback(), utc_fallback);
    assert_eq!(
        requests
            .lock()
            .expect("synthetic calendar request log mutex is not poisoned")
            .len(),
        2
    );
}

#[gpui_kit::test]
async fn unknown_event_update_requires_sync_before_another_write(cx: &mut TestAppContext) {
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
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("editor exposes the appointment title");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_keystrokes("ctrl-a");
    context.simulate_input("Uncertain appointment update");
    provider.fail_next_event_update(CalendarOperationError::UpdateOutcomeUnknown);
    let save = context
        .debug_bounds("event-composer-submit")
        .expect("editor offers a save action");
    context.simulate_click(save.center(), Modifiers::default());

    wait_for_ui_condition(context, |_, app| {
        calendar.read(app).write_readiness != CalendarWriteReadiness::Ready
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| calendar.read(cx).write_readiness != CalendarWriteReadiness::Ready)
    );
    let close = context
        .debug_bounds("event-composer-close")
        .expect("uncertain result can be dismissed");
    context.simulate_click(close.center(), Modifiers::default());
    draw_calendar_view(context);

    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("calendar keeps the new-event action visible during recovery");
    context.simulate_click(new_event.center(), Modifiers::default());
    draw_calendar_view(context);
    assert!(!context.update(|window, cx| window.has_active_sheet(cx)));

    let edit = context
        .debug_bounds("calendar-event-open")
        .expect("cached appointment remains visible until sync");
    context.simulate_click(edit.center(), Modifiers::default());
    draw_calendar_view(context);
    assert!(!context.update(|window, cx| window.has_active_sheet(cx)));
    assert_eq!(
        provider.event_update_calls.load(Ordering::SeqCst),
        1,
        "disabled Edit starts no second update"
    );

    provider.fail_next_calendar_sync(CalendarOperationError::ServiceUnavailable);
    let sync = context
        .debug_bounds("calendar-sync")
        .expect("calendar exposes a recovery sync action");
    context.simulate_click(sync.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(calendar.read(app).status, super::CalendarStatus::Failed(_))
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| calendar.read(cx).write_readiness != CalendarWriteReadiness::Ready)
    );
    let edit = context
        .debug_bounds("calendar-event-open")
        .expect("failed sync keeps the appointment visible");
    context.simulate_click(edit.center(), Modifiers::default());
    draw_calendar_view(context);
    assert!(!context.update(|window, cx| window.has_active_sheet(cx)));

    let sync = context
        .debug_bounds("calendar-sync")
        .expect("calendar keeps sync available after failure");
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
        .expect("successful sync restores Edit");
    context.simulate_click(edit.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    assert!(context.debug_bounds("event-composer-fields").is_some());
    assert_eq!(
        provider.event_update_calls.load(Ordering::SeqCst),
        1,
        "opening the editor after sync does not replay the update"
    );
}
