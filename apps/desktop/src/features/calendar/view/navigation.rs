//! What Calendar puts in the navigation pane: New event, and a month to pick a week from.

use einfach_mail_application::{CalendarSession, CalendarWriteReadiness};
use einfach_mail_domain::CalendarAction;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, IntoElement, KeyBinding, Render, Role,
    Subscription, Window, div, prelude::*, transparent_black,
};
use jiff::civil;

use crate::components::AppIcon;

use super::CalendarView;
use super::copy::{event_creation_action_copy, full_date_label, month_title};
use super::mini_month::{CursorMove, MonthGrid, is_in_week, month_of_week};
use super::render::IconControl;

gpui_kit::actions!(
    calendar_mini_month,
    [
        SelectPreviousMiniMonthDay,
        SelectNextMiniMonthDay,
        SelectPreviousMiniMonthWeek,
        SelectNextMiniMonthWeek,
        ShowPreviousMiniMonth,
        ShowNextMiniMonth,
        PickMiniMonthWeek,
    ]
);

const CALENDAR_MINI_MONTH_CONTEXT: &str = "CalendarMiniMonth";
const WEEKDAY_INITIALS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

pub(super) fn bind_mini_month_keys(cx: &mut App) {
    let context = Some(CALENDAR_MINI_MONTH_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", SelectPreviousMiniMonthDay, context),
        KeyBinding::new("right", SelectNextMiniMonthDay, context),
        KeyBinding::new("up", SelectPreviousMiniMonthWeek, context),
        KeyBinding::new("down", SelectNextMiniMonthWeek, context),
        KeyBinding::new("pageup", ShowPreviousMiniMonth, context),
        KeyBinding::new("pagedown", ShowNextMiniMonth, context),
        KeyBinding::new("enter", PickMiniMonthWeek, context),
        KeyBinding::new("space", PickMiniMonthWeek, context),
    ]);
}

/// The calendar's part of the navigation pane. It follows the calendar view: the month it shows
/// contains the selected week, and choosing a day selects that day's week.
pub(crate) struct CalendarNavigation<S: CalendarSession + ?Sized + 'static = dyn CalendarSession> {
    calendar: Entity<CalendarView<S>>,
    /// The first day of the month the mini month shows.
    month: civil::Date,
    /// The day the keyboard is on. It is also the day a Pick selects.
    cursor: civil::Date,
    /// The week this navigation last followed, so a new week is noticed once.
    followed_week: civil::Date,
    focus: FocusHandle,
    _subscription: Subscription,
}

/// The "New event" button, present only when the account can create events.
struct NewEventControl {
    label: &'static str,
    disabled: bool,
}

/// The month being drawn, with what each of its days needs to know about the frame.
struct MonthFrame<'a> {
    grid: &'a MonthGrid,
    state: &'a NavigationState,
    show_cursor: bool,
}

/// What the navigation reads from the calendar view each frame.
struct NavigationState {
    new_event: Option<NewEventControl>,
    shown_week: civil::Date,
    today: civil::Date,
}

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    fn navigation_state(&self) -> NavigationState {
        let can_create = self.supports_calendar_action(CalendarAction::CreateEvent)
            || self.pending_creation.is_some();
        NavigationState {
            new_event: can_create.then(|| NewEventControl {
                label: event_creation_action_copy(self.pending_creation.as_ref()).button_label,
                disabled: self.status.is_sync_active()
                    || self.write_readiness != CalendarWriteReadiness::Ready,
            }),
            shown_week: self.week.start(),
            today: self.clock.today(),
        }
    }
}

impl<S> CalendarNavigation<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    pub(crate) fn new(calendar: Entity<CalendarView<S>>, cx: &mut Context<Self>) -> Self {
        let week_start = calendar.read(cx).week.start();
        let subscription = cx.observe(&calendar, |navigation, calendar, cx| {
            let week_start = calendar.read(cx).week.start();
            navigation.follow_week(week_start);
            cx.notify();
        });
        Self {
            calendar,
            month: month_of_week(week_start),
            cursor: week_start,
            followed_week: week_start,
            focus: cx.focus_handle().tab_stop(true),
            _subscription: subscription,
        }
    }

    /// Moves the mini month to a newly shown week, unless the month on screen already shows it.
    fn follow_week(&mut self, week_start: civil::Date) {
        if week_start == self.followed_week {
            return;
        }
        self.followed_week = week_start;
        if !MonthGrid::containing(self.month).shows_week(week_start) {
            self.month = month_of_week(week_start);
        }
        if !is_in_week(self.cursor, week_start) {
            self.cursor = week_start;
        }
    }

    fn choose_day(&mut self, date: civil::Date, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = date;
        self.focus.focus(window, cx);
        self.calendar
            .update(cx, |calendar, cx| calendar.show_date(date, cx));
        cx.notify();
    }

    fn move_cursor(&mut self, movement: CursorMove, cx: &mut Context<Self>) {
        self.cursor = movement.apply(self.cursor);
        self.month = self.cursor.first_of_month();
        cx.notify();
    }

    fn show_month(&mut self, movement: CursorMove, cx: &mut Context<Self>) {
        self.month = movement.apply(self.month);
        cx.notify();
    }

    fn render_new_event_button(&self, control: &NewEventControl) -> impl IntoElement {
        let calendar = self.calendar.downgrade();
        div()
            .w_full()
            .debug_selector(|| "calendar-new-event".to_owned())
            .child(
                Button::new("calendar-new-event")
                    .primary()
                    .icon(Icon::new(AppIcon::NewEvent))
                    .label(control.label)
                    .w_full()
                    .disabled(control.disabled)
                    .on_click(move |_, window, cx| {
                        if let Some(calendar) = calendar.upgrade() {
                            calendar.update(cx, |view, cx| view.open_event_composer(window, cx));
                        }
                    }),
            )
    }

    fn render_month_header(&self, grid: &MonthGrid, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                div()
                    .px_2()
                    .text_sm()
                    .font_semibold()
                    .child(month_title(grid.month_start())),
            )
            .child(
                h_flex()
                    .items_center()
                    .child(self.render_month_step_button(
                        IconControl {
                            id: "calendar-mini-previous-month",
                            label: "Previous month",
                            icon: AppIcon::ChevronLeft,
                        },
                        CursorMove::PreviousMonth,
                        cx,
                    ))
                    .child(self.render_month_step_button(
                        IconControl {
                            id: "calendar-mini-next-month",
                            label: "Next month",
                            icon: AppIcon::ChevronRight,
                        },
                        CursorMove::NextMonth,
                        cx,
                    )),
            )
    }

    fn render_month_step_button(
        &self,
        control: IconControl,
        movement: CursorMove,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div().debug_selector(move || control.id.to_owned()).child(
            control.button().small().on_click(
                cx.listener(move |navigation, _, _, cx| navigation.show_month(movement, cx)),
            ),
        )
    }

    fn render_mini_month(
        &self,
        state: &NavigationState,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let grid = MonthGrid::containing(self.month);
        let frame = MonthFrame {
            grid: &grid,
            state,
            show_cursor: self.focus.is_focused(window) && window.last_input_was_keyboard(),
        };
        let mut rows = Vec::new();
        for week in grid.weeks() {
            rows.push(self.render_week_row(week, &frame, cx));
        }
        let weekday_row = h_flex().w_full().children(WEEKDAY_INITIALS.map(|initial| {
            div()
                .flex_1()
                .flex()
                .justify_center()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(initial)
        }));
        let days = v_flex()
            .id("calendar-mini-days")
            .debug_selector(|| "calendar-mini-days".to_owned())
            .role(Role::Grid)
            .aria_label(month_title(grid.month_start()));
        #[cfg(test)]
        let days = days.test_support();
        let days = days
            .key_context(CALENDAR_MINI_MONTH_CONTEXT)
            .track_focus(&self.focus);
        v_flex()
            .id("calendar-mini-month")
            .role(Role::Group)
            .aria_label("Month")
            .w_full()
            .gap_1()
            .child(self.render_month_header(&grid, cx))
            .child(weekday_row)
            .child(Self::on_cursor_actions(days, cx).w_full().children(rows))
            .into_any_element()
    }

    /// Handles the keys that move the cursor of the mini month and pick its week.
    fn on_cursor_actions<T: InteractiveElement>(element: T, cx: &mut Context<Self>) -> T {
        element
            .on_action(
                cx.listener(|navigation, _: &SelectPreviousMiniMonthDay, _, cx| {
                    navigation.move_cursor(CursorMove::PreviousDay, cx);
                }),
            )
            .on_action(
                cx.listener(|navigation, _: &SelectNextMiniMonthDay, _, cx| {
                    navigation.move_cursor(CursorMove::NextDay, cx);
                }),
            )
            .on_action(
                cx.listener(|navigation, _: &SelectPreviousMiniMonthWeek, _, cx| {
                    navigation.move_cursor(CursorMove::PreviousWeek, cx);
                }),
            )
            .on_action(
                cx.listener(|navigation, _: &SelectNextMiniMonthWeek, _, cx| {
                    navigation.move_cursor(CursorMove::NextWeek, cx);
                }),
            )
            .on_action(cx.listener(|navigation, _: &ShowPreviousMiniMonth, _, cx| {
                navigation.move_cursor(CursorMove::PreviousMonth, cx);
            }))
            .on_action(cx.listener(|navigation, _: &ShowNextMiniMonth, _, cx| {
                navigation.move_cursor(CursorMove::NextMonth, cx);
            }))
            .on_action(
                cx.listener(|navigation, _: &PickMiniMonthWeek, window, cx| {
                    navigation.choose_day(navigation.cursor, window, cx);
                }),
            )
    }

    fn render_week_row(
        &self,
        week: &[civil::Date; 7],
        frame: &MonthFrame<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_shown_week = is_in_week(week[0], frame.state.shown_week);
        let band = cx.theme().list_active;
        let mut days = Vec::with_capacity(week.len());
        for date in week {
            days.push(self.render_day(*date, in_shown_week, frame, cx));
        }
        div()
            .id(format!(
                "calendar-mini-week-{}",
                week[0].strftime("%Y-%m-%d")
            ))
            .role(Role::Row)
            .w_full()
            .flex()
            .rounded_md()
            .when(in_shown_week, |row| row.bg(band))
            .children(days)
            .into_any_element()
    }

    fn render_day(
        &self,
        date: civil::Date,
        in_shown_week: bool,
        frame: &MonthFrame<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (foreground, muted, primary, primary_foreground, hover, ring) = (
            theme.foreground,
            theme.muted_foreground,
            theme.primary,
            theme.primary_foreground,
            theme.list_hover,
            theme.ring,
        );
        let is_today = date == frame.state.today;
        let is_cursor = frame.show_cursor && date == self.cursor;
        let selector = format!("calendar-mini-day-{}", date.strftime("%Y-%m-%d"));
        let cell = div()
            .id(selector.clone())
            .debug_selector(move || selector.clone())
            .role(Role::GridCell)
            .aria_label(full_date_label(date))
            .aria_selected(in_shown_week)
            .when(is_cursor, |cell| cell.aria_active_descendant())
            .flex_1()
            .h_8()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .border_1()
            .border_color(if is_cursor { ring } else { transparent_black() })
            .text_sm()
            .text_color(if frame.grid.is_in_month(date) {
                foreground
            } else {
                muted
            })
            .when(is_today, |cell| {
                cell.bg(primary).text_color(primary_foreground)
            })
            .when(!is_today, |cell| cell.hover(move |cell| cell.bg(hover)))
            .child(date.day().to_string());
        #[cfg(test)]
        let cell = cell.test_support();
        cell.on_click(cx.listener(move |navigation, _, window, cx| {
            navigation.choose_day(date, window, cx);
        }))
        .into_any_element()
    }
}

impl<S> Render for CalendarNavigation<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.calendar.read(cx).navigation_state();
        let mini_month = self.render_mini_month(&state, window, cx);
        let navigation = v_flex()
            .id("calendar-navigation")
            .debug_selector(|| "calendar-navigation".to_owned())
            .w_full()
            .flex_1()
            .min_h_0()
            .gap_3()
            .px_2()
            .py_2()
            .when_some(state.new_event.as_ref(), |navigation, control| {
                navigation.child(self.render_new_event_button(control))
            })
            .child(mini_month);
        #[cfg(test)]
        let navigation = navigation.test_support();
        navigation
    }
}
