use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Pixels, Role};

use super::super::agenda::week_agenda;
use super::super::state::{CalendarLayout, WeekAgenda};
use super::*;
use crate::components::LocalClock;

type TestWorkflow = CalendarWorkflow<TestProvider, TestCache>;

fn at(day: u8, hour: u8) -> time::OffsetDateTime {
    Date::from_calendar_date(2030, Month::January, day)
        .expect("synthetic date is valid")
        .with_time(time::Time::from_hms(hour, 0, 0).expect("synthetic time is valid"))
        .assume_utc()
}

fn event_on(id: &str, day: u8, hour: u8) -> einfach_mail_domain::CalendarOccurrence {
    timed_event_with_id(
        id,
        at(day, hour),
        at(day, hour) + time::Duration::minutes(30),
    )
}

/// Monday 7, Wednesday 9 (twice, the later one listed first) and Sunday 13 of the week of
/// January 7, 2030.
fn week_events() -> Vec<einfach_mail_domain::CalendarOccurrence> {
    vec![
        event_on("ev-sun", 13, 12),
        event_on("ev-wed-late", 9, 10),
        event_on("ev-mon", 7, 9),
        event_on("ev-wed-early", 9, 8),
    ]
}

fn week_start() -> civil::Date {
    civil::Date::new(2030, 1, 7).expect("synthetic week is valid")
}

fn wednesday() -> civil::Date {
    civil::Date::new(2030, 1, 9).expect("synthetic day is valid")
}

fn event_ids(agenda: &WeekAgenda<'_>) -> Vec<String> {
    agenda
        .events
        .iter()
        .map(|event| event.key.event_id.as_str().to_owned())
        .collect()
}

#[test]
fn week_agenda_places_events_in_their_day_columns_in_flat_order() {
    let events = week_events();
    let agenda = week_agenda(&events, week_start(), &TimeZone::UTC, wednesday());

    assert_eq!(
        event_ids(&agenda),
        ["ev-mon", "ev-wed-early", "ev-wed-late", "ev-sun"]
    );
    let ranges = agenda
        .grid
        .days
        .iter()
        .map(|day| day.events.clone())
        .collect::<Vec<_>>();
    assert_eq!(ranges, [0..1, 1..1, 1..3, 3..3, 3..3, 3..3, 3..4]);
    let dates = agenda
        .grid
        .days
        .iter()
        .map(|day| day.date.day())
        .collect::<Vec<_>>();
    assert_eq!(dates, [7, 8, 9, 10, 11, 12, 13]);
}

#[test]
fn week_agenda_marks_only_today() {
    let events = week_events();
    let agenda = week_agenda(&events, week_start(), &TimeZone::UTC, wednesday());

    let today = agenda
        .grid
        .days
        .iter()
        .filter(|day| day.is_today)
        .map(|day| day.date)
        .collect::<Vec<_>>();
    assert_eq!(today, [wednesday()]);
    let next_week = week_agenda(
        &events,
        week_start(),
        &TimeZone::UTC,
        civil::Date::new(2030, 1, 20).expect("synthetic day is valid"),
    );
    assert!(next_week.grid.days.iter().all(|day| !day.is_today));
}

#[test]
fn scroll_index_follows_the_layout() {
    let events = week_events();
    let agenda = week_agenda(&events, week_start(), &TimeZone::UTC, wednesday());

    let grid = (0..4)
        .map(|index| agenda.scroll_index(index, CalendarLayout::Grid))
        .collect::<Vec<_>>();
    assert_eq!(grid, [0, 1, 2, 3]);
    // Monday: heading, ev-mon. Tuesday: heading, empty caption. Wednesday: heading, two events.
    // Thursday to Saturday: heading and empty caption each. Sunday: heading, ev-sun.
    let agenda_list = (0..4)
        .map(|index| agenda.scroll_index(index, CalendarLayout::Agenda))
        .collect::<Vec<_>>();
    assert_eq!(agenda_list, [1, 5, 6, 14]);
}

#[test]
fn layout_falls_back_to_the_agenda_below_the_grid_threshold() {
    let rem = px(16.);
    let navigation_pane = px(256.);
    let grid_minimum = px(704.);
    let layout_at = |window_width: Pixels| CalendarLayout::for_window_width(window_width, rem);

    assert_eq!(
        layout_at(navigation_pane + grid_minimum),
        CalendarLayout::Grid
    );
    assert_eq!(
        layout_at(navigation_pane + grid_minimum - px(1.)),
        CalendarLayout::Agenda
    );
    assert_eq!(layout_at(px(340.)), CalendarLayout::Agenda);
    assert_eq!(layout_at(px(1280.)), CalendarLayout::Grid);
}

async fn open_week_calendar(
    cx: &mut TestAppContext,
    profile: einfach_mail_domain::AccountProfile,
    window_width: f32,
) -> (Entity<CalendarView<TestWorkflow>>, &mut VisualTestContext) {
    cx.update(|app| {
        gpui_kit::init(app);
        bind_calendar_event_navigation_keys(app);
    });
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        provider,
        TestCache::with_events(week_events()),
    ));
    let mut calendar_view = None;
    let window = cx.open_window(size(px(window_width), px(800.)), |window, app| {
        let view = app.new(|app| {
            CalendarView::new_for_week(
                profile,
                workflow,
                system_calendar_week(week_start(), TimeZone::UTC),
                app,
            )
        });
        calendar_view = Some(view.clone());
        support::calendar_workspace(view, window, app)
    });
    cx.run_until_parked();
    let context = VisualTestContext::from_window(*window.deref(), cx).into_mut();
    let view = calendar_view.expect("calendar view is created with the test window");
    context.cx.update(|app| {
        view.update(app, |view, _| {
            view.clock = LocalClock::fixed(TimeZone::UTC, wednesday());
        });
    });
    wait_for_ui_condition(context, |_, app| !view.read(app).status.is_sync_active()).await;
    draw_calendar_view(context);
    (view, context)
}

fn selected_event_id(
    context: &mut VisualTestContext,
    view: &Entity<CalendarView<TestWorkflow>>,
) -> Option<String> {
    context.cx.update(|app| {
        view.read(app)
            .selected_agenda_event
            .as_ref()
            .map(|key| key.event_id.as_str().to_owned())
    })
}

fn focus_event_list(context: &mut VisualTestContext, view: &Entity<CalendarView<TestWorkflow>>) {
    context.update(|window, app| {
        let focus = view.read(app).event_list_focus.clone();
        focus.focus(window, app);
    });
}

#[gpui_kit::test]
async fn grid_draws_each_event_under_its_day_header(cx: &mut TestAppContext) {
    let (_, context) = open_week_calendar(cx, profile(), 1280.).await;

    for (day, header_selector, event_id, event_selector) in [
        (
            7,
            "calendar-day-header-2030-01-07",
            "ev-mon",
            "calendar-event-row-2030-01-07-ev-mon",
        ),
        (
            9,
            "calendar-day-header-2030-01-09",
            "ev-wed-early",
            "calendar-event-row-2030-01-09-ev-wed-early",
        ),
        (
            9,
            "calendar-day-header-2030-01-09",
            "ev-wed-late",
            "calendar-event-row-2030-01-09-ev-wed-late",
        ),
        (
            13,
            "calendar-day-header-2030-01-13",
            "ev-sun",
            "calendar-event-row-2030-01-13-ev-sun",
        ),
    ] {
        let header = context
            .debug_bounds(header_selector)
            .unwrap_or_else(|| panic!("the header of January {day} is drawn"));
        let event = context
            .debug_bounds(event_selector)
            .unwrap_or_else(|| panic!("{event_id} is drawn"));
        assert!(
            event.left() >= header.left() && event.right() <= header.right(),
            "{event_id} stays inside the column of January {day}; event={event:?}, header={header:?}"
        );
        assert!(
            event.top() >= header.bottom(),
            "{event_id} sits below the day headers"
        );
    }
    let early = context
        .debug_bounds("calendar-event-row-2030-01-09-ev-wed-early")
        .expect("the earlier Wednesday event is drawn");
    let late = context
        .debug_bounds("calendar-event-row-2030-01-09-ev-wed-late")
        .expect("the later Wednesday event is drawn");
    assert!(
        early.bottom() <= late.top(),
        "events of one day stack by time"
    );
}

#[gpui_kit::test]
async fn grid_keyboard_order_crosses_columns_day_by_day(cx: &mut TestAppContext) {
    let (view, context) = open_week_calendar(cx, profile(), 1280.).await;
    focus_event_list(context, &view);

    let mut visited = Vec::new();
    for _ in 0..4 {
        context.simulate_keystrokes("down");
        visited.push(selected_event_id(context, &view).expect("Down selects an event"));
    }
    assert_eq!(visited, ["ev-mon", "ev-wed-early", "ev-wed-late", "ev-sun"]);

    context.simulate_keystrokes("down");
    assert_eq!(selected_event_id(context, &view).as_deref(), Some("ev-sun"));
    context.simulate_keystrokes("up");
    assert_eq!(
        selected_event_id(context, &view).as_deref(),
        Some("ev-wed-late")
    );
    context.simulate_keystrokes("home");
    assert_eq!(selected_event_id(context, &view).as_deref(), Some("ev-mon"));
    context.simulate_keystrokes("end");
    assert_eq!(selected_event_id(context, &view).as_deref(), Some("ev-sun"));
}

#[gpui_kit::test]
async fn grid_listbox_holds_only_events_as_options(cx: &mut TestAppContext) {
    let (_, context) = open_week_calendar(cx, profile(), 1280.).await;

    let (listboxes, options, groups) = context.update(|window, _| {
        (
            window.find_all_by_role(Role::ListBox),
            window.find_all_by_role(Role::ListBoxOption),
            window.find_all_by_role(Role::Group),
        )
    });
    assert_eq!(listboxes.len(), 1, "the grid is still one listbox");
    assert_eq!(options.len(), 4, "only events are options");
    let day_headers = groups
        .iter()
        .filter(|group| {
            group
                .label()
                .is_some_and(|label| label.contains("January") && !label.contains("Active"))
        })
        .count();
    assert!(
        day_headers >= 7,
        "the seven day headers exist and are not options; groups: {:?}",
        groups.iter().map(|group| group.label()).collect::<Vec<_>>()
    );
}

#[gpui_kit::test]
async fn grid_names_and_highlights_today(cx: &mut TestAppContext) {
    let (_, context) = open_week_calendar(cx, profile(), 1280.).await;

    let today_labels = context.update(|window, _| {
        window
            .find_all_by_role(Role::Group)
            .iter()
            .filter_map(|group| group.label().map(str::to_owned))
            .filter(|label| label.ends_with(", today"))
            .collect::<Vec<_>>()
    });
    assert_eq!(today_labels, ["Wednesday, January 9, today"]);
    assert!(
        context
            .debug_bounds("calendar-day-number-2030-01-09")
            .is_some()
    );
}

#[gpui_kit::test]
async fn narrow_window_shows_the_agenda_with_the_same_options(cx: &mut TestAppContext) {
    let (view, context) = open_week_calendar(cx, profile(), 700.).await;

    assert_eq!(
        context.cx.update(|app| view.read(app).layout),
        CalendarLayout::Agenda
    );
    assert!(
        context
            .debug_bounds("calendar-day-number-2030-01-09")
            .is_none(),
        "the agenda has no day columns"
    );
    assert!(
        context
            .debug_bounds("calendar-day-header-2030-01-09")
            .is_some(),
        "the agenda groups events under day headings"
    );
    let options = context.update(|window, _| window.find_all_by_role(Role::ListBoxOption));
    assert_eq!(options.len(), 4);
    focus_event_list(context, &view);
    context.simulate_keystrokes("down down down");
    assert_eq!(
        selected_event_id(context, &view).as_deref(),
        Some("ev-wed-late"),
        "the agenda uses the same flat order as the grid"
    );
}

#[gpui_kit::test]
async fn mini_month_selects_the_week_of_the_chosen_day(cx: &mut TestAppContext) {
    let (view, context) = open_week_calendar(cx, profile(), 1280.).await;

    let day = context
        .debug_bounds("calendar-mini-day-2030-01-17")
        .expect("the mini month shows January 17");
    context.simulate_click(day.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        view.read(app).week.start() == civil::Date::new(2030, 1, 14).expect("valid date")
            && !view.read(app).status.is_sync_active()
    })
    .await;
    draw_calendar_view(context);

    assert!(
        context
            .debug_bounds("calendar-day-header-2030-01-14")
            .is_some(),
        "the grid shows the week of the chosen day"
    );
    let selected_days = context.update(|window, _| {
        window
            .find_all_by_role(Role::GridCell)
            .iter()
            .filter(|cell| cell.selected() == Some(true))
            .filter_map(|cell| cell.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert_eq!(selected_days.len(), 7, "the chosen week is selected");
    assert!(
        selected_days
            .iter()
            .all(|label| label.contains("January 14")
                || label.contains("January 15")
                || label.contains("January 16")
                || label.contains("January 17")
                || label.contains("January 18")
                || label.contains("January 19")
                || label.contains("January 20")),
        "selected cells are exactly January 14 to 20: {selected_days:?}"
    );
}

#[gpui_kit::test]
async fn mini_month_picks_a_week_by_keyboard(cx: &mut TestAppContext) {
    let (view, context) = open_week_calendar(cx, read_only_calendar_profile(), 1280.).await;

    context.update(gpui_kit::Window::focus_next);
    for _ in 0..12 {
        if context.update(|window, _| window.find("calendar-mini-days").focused()) == Some(true) {
            break;
        }
        context.simulate_keystrokes("tab");
    }
    assert_eq!(
        context.update(|window, _| window.find("calendar-mini-days").focused()),
        Some(true),
        "Tab reaches the mini month"
    );

    context.simulate_keystrokes("down enter");
    wait_for_ui_condition(context, |_, app| {
        view.read(app).week.start() == civil::Date::new(2030, 1, 14).expect("valid date")
            && !view.read(app).status.is_sync_active()
    })
    .await;
}

#[gpui_kit::test]
async fn previous_next_and_today_move_between_weeks(cx: &mut TestAppContext) {
    let (view, context) = open_week_calendar(cx, profile(), 1280.).await;
    let shown_week =
        |context: &mut VisualTestContext| context.cx.update(|app| view.read(app).week.start());
    let click = |context: &mut VisualTestContext, selector: &'static str| {
        let bounds = context
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"));
        context.simulate_click(bounds.center(), Modifiers::default());
    };
    let today_button_disabled = |context: &mut VisualTestContext| {
        context.update(|window, _| {
            window
                .find_all_by_role(Role::Button)
                .iter()
                .find(|button| button.label() == Some("Today"))
                .map(|button| button.disabled() == Some(true))
        })
    };

    assert_eq!(
        today_button_disabled(context),
        Some(true),
        "Today is unavailable while the shown week holds today"
    );

    click(context, "calendar-next-week");
    wait_for_ui_condition(context, |_, app| {
        view.read(app).week.start() == civil::Date::new(2030, 1, 14).expect("valid date")
            && !view.read(app).status.is_sync_active()
    })
    .await;
    draw_calendar_view(context);
    assert_eq!(today_button_disabled(context), Some(false));

    click(context, "calendar-previous-week");
    click(context, "calendar-previous-week");
    wait_for_ui_condition(context, |_, app| {
        view.read(app).week.start() == civil::Date::new(2029, 12, 31).expect("valid date")
            && !view.read(app).status.is_sync_active()
    })
    .await;
    draw_calendar_view(context);

    click(context, "calendar-today");
    wait_for_ui_condition(context, |_, app| {
        view.read(app).week.start() == week_start() && !view.read(app).status.is_sync_active()
    })
    .await;
    draw_calendar_view(context);
    assert_eq!(shown_week(context), week_start());
    assert_eq!(today_button_disabled(context), Some(true));
}

#[gpui_kit::test]
async fn new_event_sits_in_the_navigation_pane_and_follows_capabilities(cx: &mut TestAppContext) {
    let (_, context) = open_week_calendar(cx, profile(), 1280.).await;
    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("a calendar that can create events offers New event");
    let navigation = context
        .debug_bounds("calendar-navigation")
        .expect("the calendar contributes to the navigation pane");
    assert!(
        new_event.left() >= navigation.left() && new_event.right() <= navigation.right(),
        "New event is in the navigation pane; button={new_event:?}, pane={navigation:?}"
    );
    assert!(
        context.debug_bounds("calendar-mini-days").is_some(),
        "the mini month sits with it"
    );
}
