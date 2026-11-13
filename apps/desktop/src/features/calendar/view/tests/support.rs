use super::super::super::week::CalendarWeek;
use super::super::{CalendarNavigation, CalendarView};
use crate::test_support::calendar::profile;
use einfach_mail_application::CalendarSession;
use einfach_mail_domain::CalendarViewReference;
use gpui_kit::{Context, Entity, IntoElement, Render, Window, base::Root, div, prelude::*, rems};
use jiff::{civil, tz::TimeZone};

pub(super) fn system_calendar_week(week_start: civil::Date, time_zone: TimeZone) -> CalendarWeek {
    CalendarWeek::in_system_zone(profile().id(), week_start, time_zone)
        .expect("synthetic system calendar week is valid")
}

pub(super) fn utc_fallback_calendar_week(week_start: civil::Date) -> CalendarWeek {
    CalendarWeek::in_utc_fallback(profile().id(), week_start)
        .expect("synthetic UTC fallback calendar week is valid")
}

pub(super) fn system_calendar_view(week_start: civil::Date) -> CalendarViewReference {
    system_calendar_week(week_start, TimeZone::UTC)
        .view()
        .clone()
}

/// The calendar view beside its navigation, in the arrangement the workspace gives them.
struct CalendarWorkspace<S: CalendarSession + ?Sized + 'static> {
    navigation: Entity<CalendarNavigation<S>>,
    calendar: Entity<CalendarView<S>>,
}

impl<S: CalendarSession + ?Sized + 'static> Render for CalendarWorkspace<S> {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .w(rems(16.))
                    .h_full()
                    .flex_shrink_0()
                    .child(self.navigation.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.calendar.clone()),
            )
    }
}

/// Mounts `calendar` with its navigation as the root of a test window.
pub(super) fn calendar_workspace<S: CalendarSession + ?Sized + 'static>(
    calendar: Entity<CalendarView<S>>,
    window: &mut Window,
    cx: &mut Context<Root>,
) -> Root {
    let navigation = cx.new(|cx| CalendarNavigation::new(calendar.clone(), cx));
    let workspace = cx.new(|_| CalendarWorkspace {
        navigation,
        calendar,
    });
    Root::new(workspace, window, cx)
}
