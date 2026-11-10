use std::{
    ops::Deref,
    sync::{Arc, atomic::Ordering},
};

use einfach_mail_application::{
    CalendarOperationError, CalendarSession, CalendarWorkflow, CalendarWorkflowError,
    CalendarWriteReadiness,
};
use einfach_mail_domain::{CalendarOccurrenceTime, EventId};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext, px, size};
use jiff::{Span, civil, tz::TimeZone};
use time::{Date, Month};

use super::super::support::{system_calendar_week, utc_fallback_calendar_week};
use super::{CalendarStatus, CalendarView};
use crate::test_support::calendar::{
    TestCache, TestProvider, agenda_events, draw_calendar_view, profile,
    read_only_calendar_profile, select_default_timed_event, timed_event, timed_event_with_id,
    workflow_account_id,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

#[derive(Clone, Copy)]
enum ExpectedEventDetailsLayout {
    Stacked,
    Inline,
}

#[gpui_kit::test]
async fn native_event_composer_renders_date_pickers_and_recovers_create_outcomes(
    cx: &mut TestAppContext,
) {
    super::composer_tests::run_native_event_composer_renders_date_pickers_and_recovers_create_outcomes(cx).await;
}

#[gpui_kit::test]
async fn denied_calendar_write_emits_permission_review_request(cx: &mut TestAppContext) {
    super::composer_tests::run_denied_calendar_write_emits_permission_review_request(cx).await;
}

#[gpui_kit::test]
async fn native_event_editor_updates_a_cached_timed_appointment(cx: &mut TestAppContext) {
    super::composer_tests::run_native_event_editor_updates_a_cached_timed_appointment(cx).await;
}

#[gpui_kit::test]
async fn native_event_editor_confirms_and_serializes_deletion(cx: &mut TestAppContext) {
    super::composer_tests::run_native_event_editor_confirms_and_serializes_deletion(cx).await;
}

#[test]
fn sync_activity_matches_calendar_status() {
    assert!(CalendarStatus::LoadingCache.is_sync_active());
    assert!(CalendarStatus::Syncing.is_sync_active());
    assert!(!CalendarStatus::Idle.is_sync_active());
    assert!(!CalendarStatus::UpToDate.is_sync_active());
    assert!(!CalendarStatus::Failed(CalendarWorkflowError::AccountMismatch).is_sync_active());
}

#[gpui_kit::test]
async fn compact_calendar_keeps_navigation_and_agenda_inside_the_available_width(
    cx: &mut TestAppContext,
) {
    cx.update(|app| {
        gpui_kit::init(app);
        super::bind_calendar_event_navigation_keys(app);
    });

    for (width, expected_details_layout) in [
        (340.0, ExpectedEventDetailsLayout::Stacked),
        (520.0, ExpectedEventDetailsLayout::Inline),
    ] {
        assert_calendar_layout_at_width(cx, width, expected_details_layout).await;
    }
}

async fn assert_calendar_layout_at_width(
    cx: &mut TestAppContext,
    width: f32,
    expected_details_layout: ExpectedEventDetailsLayout,
) {
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        TestProvider::default(),
        TestCache::default(),
    ));
    let mut calendar_view = None;
    let window = cx.open_window(size(px(width), px(640.)), |window, app| {
        let view = app.new(|app| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                system_calendar_week(week_start, TimeZone::UTC),
                app,
            )
        });
        calendar_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, app)
    });
    cx.run_until_parked();
    let context = VisualTestContext::from_window(*window.deref(), cx).into_mut();
    let view = calendar_view.expect("calendar view is created with the test window");

    wait_for_ui_condition(context, |_, app| !view.read(app).status.is_sync_active()).await;
    draw_calendar_view(context);
    assert_compact_agenda_accessibility(context);
    assert_calendar_controls_fit(context, width, expected_details_layout);
    assert_compact_agenda_navigation(context, &view, width);
}

fn assert_compact_agenda_accessibility(context: &mut VisualTestContext) {
    let listboxes = context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBox));
    let options =
        context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBoxOption));
    assert_eq!(
        listboxes.len(),
        1,
        "the agenda keeps one listbox focus stop"
    );
    assert_eq!(listboxes[0].label(), Some("Events this week"));
    assert_eq!(options.len(), 2, "both cached events remain reachable");
    let labels = options
        .iter()
        .filter_map(|option| option.label())
        .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| {
        label.contains("Synthetic offline event") && label.contains("Synthetic room")
    }));
    assert!(
        labels
            .iter()
            .any(|label| label.contains("Synthetic overnight event"))
    );
}

fn assert_calendar_controls_fit(
    context: &mut VisualTestContext,
    width: f32,
    expected_details_layout: ExpectedEventDetailsLayout,
) {
    let viewport = context.update(|window, _| window.bounds());
    for selector in [
        "calendar-sync",
        "calendar-new-event",
        "calendar-week-navigation",
        "calendar-previous-week",
        "calendar-week-label",
        "calendar-next-week",
        "calendar-event-list",
        "calendar-event-row-2030-01-07-synthetic-ui-event",
        "calendar-event-time-2030-01-07-synthetic-ui-event",
        "calendar-event-details-2030-01-07-synthetic-ui-event",
    ] {
        let bounds = context
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is rendered at {width}px"));
        assert!(
            bounds.left() >= viewport.left() && bounds.right() <= viewport.right(),
            "{selector} stays inside the {width}px calendar viewport; bounds={bounds:?}, viewport={viewport:?}"
        );
    }

    let navigation = context
        .debug_bounds("calendar-week-navigation")
        .expect("week navigation is rendered");
    let label = context
        .debug_bounds("calendar-week-label")
        .expect("the current week remains visible");
    assert!(
        label.left() >= navigation.left() && label.right() <= navigation.right(),
        "the week label stays inside its navigation row at {width}px"
    );

    let event_row = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-ui-event")
        .expect("the event row is rendered");
    let event_time = context
        .debug_bounds("calendar-event-time-2030-01-07-synthetic-ui-event")
        .expect("the event time is rendered");
    let event_details = context
        .debug_bounds("calendar-event-details-2030-01-07-synthetic-ui-event")
        .expect("the event subject and location are rendered");
    assert!(event_time.left() >= event_row.left() && event_time.right() <= event_row.right());
    assert!(event_details.left() >= event_row.left() && event_details.right() <= event_row.right());
    match expected_details_layout {
        ExpectedEventDetailsLayout::Stacked => assert!(
            event_time.bottom() <= event_details.top(),
            "event details wrap below the time at {width}px"
        ),
        ExpectedEventDetailsLayout::Inline => assert!(
            event_time.right() <= event_details.left(),
            "event details stay beside the time at {width}px"
        ),
    }
}

fn assert_compact_agenda_navigation<S: CalendarSession + ?Sized + 'static>(
    context: &mut VisualTestContext,
    view: &Entity<CalendarView<S>>,
    width: f32,
) {
    context.update(gpui_kit::Window::focus_next);
    let mut list_focused =
        context.update(|window, app| view.read(app).event_list_focus.is_focused(window));
    for _ in 0..24 {
        if list_focused {
            break;
        }
        context.simulate_keystrokes("tab");
        list_focused =
            context.update(|window, app| view.read(app).event_list_focus.is_focused(window));
    }
    assert!(list_focused, "Tab reaches the agenda at {width}px");

    context.simulate_keystrokes("down");
    assert_selected_event(context, view, "synthetic-ui-event");
    context.simulate_keystrokes("end");
    assert_selected_event(context, view, "synthetic-timed-event");
    context.simulate_keystrokes("home");
    assert_selected_event(context, view, "synthetic-ui-event");
    context.simulate_keystrokes("up");
    assert_selected_event(context, view, "synthetic-ui-event");

    let first_event = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-ui-event")
        .expect("the first event remains reachable");
    context.simulate_click(first_event.center(), Modifiers::default());
    assert!(context.update(|window, app| { view.read(app).event_list_focus.is_focused(window) }));
}

#[gpui_kit::test]
async fn read_only_calendar_keeps_agenda_navigation_and_blocks_event_writes(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        super::bind_calendar_event_navigation_keys(cx);
    });
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    let event_creation_count = Arc::clone(&provider.event_creation_count);
    let event_update_calls = Arc::clone(&provider.event_update_calls);
    let event_deletion_calls = Arc::clone(&provider.event_deletion_calls);
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        provider,
        TestCache::with_events(agenda_events(1)),
    ));
    let (view, context) = cx.add_window_view(|_, cx| {
        CalendarView::new_for_week(
            read_only_calendar_profile(),
            workflow,
            system_calendar_week(week_start, TimeZone::UTC),
            cx,
        )
    });

    context.run_until_parked();
    wait_for_ui_condition(context, |_, app| !view.read(app).status.is_sync_active()).await;
    draw_calendar_view(context);

    let status_labels = context.update(|window, _| {
        window
            .find_all_by_role(gpui_kit::Role::Status)
            .iter()
            .map(|status| status.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(
        status_labels
            .iter()
            .any(|label| label.as_deref() == Some("Calendar is read-only")),
        "calendar status labels: {status_labels:?}"
    );
    assert!(context.debug_bounds("calendar-new-event").is_none());
    assert!(context.debug_bounds("calendar-sync").is_some());
    assert!(context.debug_bounds("calendar-previous-week").is_some());
    assert!(context.debug_bounds("calendar-next-week").is_some());

    let event = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-1")
        .expect("the cached agenda renders its event");
    context.simulate_click(event.center(), Modifiers::default());
    context.run_until_parked();
    draw_calendar_view(context);
    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert!(context.debug_bounds("calendar-event-edit").is_none());

    context.simulate_keystrokes("enter");
    context.run_until_parked();
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-composer-fields").is_none());
    assert_selected_event(context, &view, "synthetic-calendar-event-1");

    context.update(|window, app| {
        view.update(app, |view, app| view.open_event_composer(window, app));
    });
    draw_calendar_view(context);
    assert!(context.debug_bounds("event-composer-fields").is_none());
    assert_eq!(event_creation_count.load(Ordering::SeqCst), 0);
    assert_eq!(event_update_calls.load(Ordering::SeqCst), 0);
    assert_eq!(event_deletion_calls.load(Ordering::SeqCst), 0);

    let next_week = context
        .debug_bounds("calendar-next-week")
        .expect("read-only calendar keeps week navigation available");
    context.simulate_click(next_week.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        view.read(app).week.start() == civil::Date::new(2030, 1, 14).expect("next week is valid")
            && !view.read(app).status.is_sync_active()
    })
    .await;
}

#[test]
fn timed_events_that_cross_midnight_appear_on_each_occupied_day() {
    let event = timed_event(
        Date::from_calendar_date(2030, Month::January, 6)
            .expect("synthetic date is valid")
            .with_time(time::Time::from_hms(23, 0, 0).expect("synthetic time is valid"))
            .assume_utc(),
        Date::from_calendar_date(2030, Month::January, 8)
            .expect("synthetic date is valid")
            .with_time(time::Time::from_hms(10, 0, 0).expect("synthetic time is valid"))
            .assume_utc(),
    );
    let time_zone = TimeZone::UTC;
    let first_day = civil::Date::new(2030, 1, 7).expect("synthetic date is valid");
    let last_day = civil::Date::new(2030, 1, 8).expect("synthetic date is valid");

    let first_day_events =
        super::display_events(std::slice::from_ref(&event), first_day, &time_zone);
    let last_day_events = super::display_events(std::slice::from_ref(&event), last_day, &time_zone);

    assert_eq!(first_day_events.len(), 1);
    assert_eq!(first_day_events[0].time_label, "Continues");
    assert_eq!(last_day_events.len(), 1);
    assert_eq!(last_day_events[0].time_label, "Until 10:00 UTC");
    assert!(matches!(event.time(), CalendarOccurrenceTime::Timed { .. }));
}

#[gpui_kit::test]
async fn calendar_agenda_navigates_events_with_one_focus_stop_and_keeps_selection_visible(
    cx: &mut TestAppContext,
) {
    const AGENDA_EVENT_COUNT: usize = 20;
    const FIRST_OPTION_LABEL: &str =
        "Monday, January 7, 08:00 UTC–08:30 UTC, Synthetic overnight event";
    const SECOND_OPTION_LABEL: &str =
        "Monday, January 7, 08:05 UTC–08:35 UTC, Synthetic overnight event";
    const LAST_OPTION_LABEL: &str =
        "Monday, January 7, 09:35 UTC–10:05 UTC, Synthetic overnight event";

    cx.update(|cx| {
        gpui_kit::init(cx);
        super::bind_calendar_event_navigation_keys(cx);
    });
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        TestProvider::default(),
        TestCache::with_events(agenda_events(AGENDA_EVENT_COUNT)),
    ));
    let mut calendar_view = None;
    let window = cx.open_window(size(px(1280.), px(800.)), |window, cx| {
        let view = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                system_calendar_week(week_start, TimeZone::UTC),
                cx,
            )
        });
        calendar_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    cx.run_until_parked();
    let context = VisualTestContext::from_window(*window.deref(), cx).into_mut();
    let view = calendar_view.expect("calendar view is created with the test window");

    wait_for_ui_condition(context, |_, app| !view.read(app).status.is_sync_active()).await;
    assert!(
        context.debug_bounds("calendar-utc-fallback").is_none(),
        "system UTC does not show the missing-time-zone fallback status"
    );
    assert_accessible_agenda(context, AGENDA_EVENT_COUNT);
    assert!(context.debug_bounds("calendar-event-list").is_some());
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).selected_agenda_event.is_none())
    );
    assert!(context.debug_bounds("calendar-event-edit").is_none());

    context.update(gpui_kit::Window::focus_next);
    let mut list_focused =
        context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window));
    for _ in 0..24 {
        if list_focused {
            break;
        }
        context.simulate_keystrokes("tab");
        list_focused =
            context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window));
        if list_focused {
            break;
        }
    }
    assert!(list_focused, "Tab reaches the calendar event list");
    assert_eq!(accessible_agenda_list_focus(context), Some(true));

    context.simulate_keystrokes("up");
    assert_selected_event(context, &view, "synthetic-calendar-event-20");
    assert_accessible_selection(context, LAST_OPTION_LABEL);
    let edit_bounds = context
        .debug_bounds("calendar-event-edit")
        .expect("selecting a timed event exposes its edit command");
    let list_bounds = context
        .debug_bounds("calendar-event-list")
        .expect("event list is rendered");
    assert!(
        edit_bounds.bottom() <= list_bounds.top(),
        "the edit command stays outside listbox options; edit={edit_bounds:?}, list={list_bounds:?}"
    );
    context.simulate_keystrokes("down");
    assert_selected_event(context, &view, "synthetic-calendar-event-20");
    assert_accessible_selection(context, LAST_OPTION_LABEL);

    context.simulate_keystrokes("home");
    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert_accessible_selection(context, FIRST_OPTION_LABEL);
    context.simulate_keystrokes("up");
    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert_accessible_selection(context, FIRST_OPTION_LABEL);
    context.simulate_keystrokes("down");
    assert_selected_event(context, &view, "synthetic-calendar-event-2");
    assert_accessible_selection(context, SECOND_OPTION_LABEL);
    context.simulate_keystrokes("end");
    context.run_until_parked();
    draw_calendar_view(context);
    assert_selected_event(context, &view, "synthetic-calendar-event-20");
    assert_accessible_selection(context, LAST_OPTION_LABEL);

    let list_bounds = context
        .debug_bounds("calendar-event-list")
        .expect("event list is rendered");
    let selected_bounds = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-20")
        .expect("End scrolls the last event into view");
    assert!(
        selected_bounds.top() >= list_bounds.top(),
        "selected event starts above the agenda viewport; list={list_bounds:?}, selected={selected_bounds:?}"
    );
    assert!(
        selected_bounds.bottom() <= list_bounds.bottom(),
        "selected event ends below the agenda viewport; list={list_bounds:?}, selected={selected_bounds:?}"
    );
    let (scroll_offset, max_scroll_offset) = context.cx.update(|cx| {
        let scroll = &view.read(cx).agenda_scroll;
        (scroll.offset(), scroll.max_offset())
    });
    assert!(
        max_scroll_offset.y > px(0.),
        "the short test viewport should require agenda scrolling; max={max_scroll_offset:?}, list={list_bounds:?}"
    );
    assert!(
        scroll_offset.y < px(0.),
        "End should scroll the agenda down; offset={scroll_offset:?}, max={max_scroll_offset:?}"
    );

    context.cx.update(|cx| {
        view.update(cx, |view, _| view.agenda_scroll.scroll_to_item(0));
    });
    context.run_until_parked();
    draw_calendar_view(context);
    context.simulate_keystrokes("tab");
    assert!(
        !context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window)),
        "Tab leaves the event list as one focus stop"
    );
    assert_eq!(accessible_agenda_list_focus(context), Some(false));
    let first_row = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-1")
        .expect("the first event is visible after Home");
    context.simulate_click(first_row.center(), Modifiers::default());
    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert!(context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window)));
    assert_eq!(accessible_agenda_list_focus(context), Some(true));
    assert_accessible_selection(context, FIRST_OPTION_LABEL);
}

#[gpui_kit::test]
async fn selecting_an_event_uses_its_current_row_after_agenda_rows_shift(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        TestProvider::default(),
        TestCache::with_events(agenda_events(20)),
    ));
    let mut calendar_view = None;
    let window = cx.open_window(size(px(1280.), px(480.)), |window, cx| {
        let view = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                system_calendar_week(week_start, TimeZone::UTC),
                cx,
            )
        });
        calendar_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    cx.run_until_parked();
    let context = VisualTestContext::from_window(*window.deref(), cx).into_mut();
    let view = calendar_view.expect("calendar view is created with the test window");

    wait_for_ui_condition(context, |_, app| !view.read(app).status.is_sync_active()).await;
    draw_calendar_view(context);
    assert!(
        context
            .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-1")
            .is_some(),
        "the first event is rendered before the rows change"
    );
    let target_event = super::AgendaEventKey {
        date: civil::Date::new(2030, 1, 7).expect("synthetic event date is valid"),
        event_id: EventId::try_from("synthetic-calendar-event-1".to_owned())
            .expect("synthetic event ID is valid"),
    };
    let old_scroll_index = context.cx.update(|cx| {
        view.read(cx).agenda_rows().iter().position(
            |row| matches!(row, super::AgendaRow::Event(event) if event.key == target_event),
        )
    });
    let old_scroll_index = old_scroll_index.expect("the target event has an agenda row");
    let date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic event date is valid");
    let earlier_start = date
        .midnight()
        .assume_utc()
        .replace_time(time::Time::from_hms(6, 0, 0).expect("synthetic time is valid"));
    let earlier_events = (0..24)
        .map(|index| {
            let minutes = i64::from(index);
            let start = earlier_start + time::Duration::minutes(minutes);
            let event_id = format!("synthetic-inserted-event-{index}");
            timed_event_with_id(&event_id, start, start + time::Duration::minutes(15))
        })
        .collect::<Vec<_>>();

    context.cx.update(|cx| {
        view.update(cx, |view, _| {
            view.events.extend(earlier_events);
        });
    });
    let current_scroll_index = context.cx.update(|cx| {
        view.read(cx).agenda_rows().iter().position(
            |row| matches!(row, super::AgendaRow::Event(event) if event.key == target_event),
        )
    });
    let current_scroll_index =
        current_scroll_index.expect("the target event remains in the changed agenda");
    assert_eq!(current_scroll_index, old_scroll_index + 24);
    context.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.select_event_from_option(target_event, window, cx);
        });
    });
    context.run_until_parked();
    draw_calendar_view(context);

    assert_selected_event(context, &view, "synthetic-calendar-event-1");
    assert!(context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window)));
    let list_bounds = context
        .debug_bounds("calendar-event-list")
        .expect("event list remains rendered");
    let selected_bounds = context
        .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-1")
        .expect("the selected event remains rendered");
    assert!(
        selected_bounds.top() >= list_bounds.top()
            && selected_bounds.bottom() <= list_bounds.bottom(),
        "selecting after earlier rows are inserted scrolls the event into view; list={list_bounds:?}, selected={selected_bounds:?}"
    );
    let scroll_offset = context.cx.update(|cx| view.read(cx).agenda_scroll.offset());
    assert!(
        scroll_offset.y < px(0.),
        "the shifted event row requires scrolling; offset={scroll_offset:?}"
    );
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
            .event_composer_setup(super::super::composer::EventComposerIntent::Create {
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
    cx.update(super::super::composer::bind_event_composer_keys);
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
        .debug_bounds("calendar-event-edit")
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
        .debug_bounds("calendar-event-edit")
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
        .debug_bounds("calendar-event-edit")
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
        .debug_bounds("calendar-event-edit")
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

fn assert_accessible_agenda(context: &mut VisualTestContext, expected_event_count: usize) {
    let (lists, options) = context.update(|window, _| {
        (
            window.find_all_by_role(gpui_kit::Role::ListBox),
            window.find_all_by_role(gpui_kit::Role::ListBoxOption),
        )
    });
    assert_eq!(lists.len(), 1, "the agenda has one listbox");
    assert_eq!(lists[0].label(), Some("Events this week"));
    assert_eq!(options.len(), expected_event_count, "agenda option count");
    assert!(
        options.iter().all(|option| option
            .label()
            .is_some_and(|label| label.contains("Synthetic overnight event"))),
        "every option exposes its event label"
    );
    assert!(
        options
            .iter()
            .all(|option| option.selected() == Some(false))
    );
}

fn accessible_agenda_list_focus(context: &mut VisualTestContext) -> Option<bool> {
    let lists = context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBox));
    assert_eq!(lists.len(), 1, "the agenda has one listbox");
    lists[0].focused()
}

fn assert_accessible_selection(context: &mut VisualTestContext, expected_label: &str) {
    let options =
        context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBoxOption));
    let selected_options: Vec<_> = options
        .iter()
        .filter(|option| option.selected() == Some(true))
        .collect();
    assert_eq!(
        selected_options.len(),
        1,
        "exactly one calendar option is exposed as selected"
    );
    assert_eq!(selected_options[0].label(), Some(expected_label));
}

fn assert_selected_event<S>(
    context: &mut VisualTestContext,
    view: &gpui_kit::Entity<CalendarView<S>>,
    expected: &str,
) where
    S: CalendarSession + ?Sized + 'static,
{
    let selected = context.cx.update(|cx| {
        view.read(cx)
            .selected_agenda_event
            .as_ref()
            .map(|key| key.event_id.as_str().to_owned())
    });
    assert_eq!(selected.as_deref(), Some(expected));
}
