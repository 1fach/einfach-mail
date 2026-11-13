//! The week as seven day columns: a header row with the days, and event bars below them.

use einfach_mail_application::CalendarSession;
use gpui_kit::component::{ActiveTheme, StyledExt, v_flex};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, Context, Hsla, IntoElement, Role, div, prelude::*, px, transparent_black,
};

use super::CalendarView;
use super::copy::{day_header_label, weekday_abbreviation};
use super::render::{EventPresentation, render_empty_agenda};
use super::state::{DayColumn, WeekAgenda, WeekGrid};

/// The strength of the tint that marks today's column.
const TODAY_COLUMN_TINT_OPACITY: f32 = 0.06;

/// The colors the grid takes from the theme.
#[derive(Clone, Copy)]
struct GridColors {
    border: Hsla,
    muted_foreground: Hsla,
    primary: Hsla,
    primary_foreground: Hsla,
}

impl GridColors {
    fn from_theme(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            border: theme.border,
            muted_foreground: theme.muted_foreground,
            primary: theme.primary,
            primary_foreground: theme.primary_foreground,
        }
    }
}

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    pub(super) fn render_week_grid(
        &self,
        agenda: &WeekAgenda<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = GridColors::from_theme(cx);
        let body = if agenda.events.is_empty() {
            let message = render_empty_agenda(self.status, cx.theme().semantic_tokens().colors);
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(render_column_backdrop(&agenda.grid, colors))
                .child(message)
                .into_any_element()
        } else {
            let events = self.render_grid_events(agenda, cx);
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(render_column_backdrop(&agenda.grid, colors))
                .child(events)
                .into_any_element()
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(render_day_headers(&agenda.grid, colors))
            .child(body)
            .into_any_element()
    }

    /// The week's listbox laid out as a grid. Each event sits in its day's column and in the next
    /// free row of that column. The listbox children are the events in their flat order, so the
    /// grid changes where an option is drawn and nothing about how it is reached.
    fn render_grid_events(&self, agenda: &WeekAgenda<'_>, cx: &mut Context<Self>) -> AnyElement {
        let mut cells = Vec::with_capacity(agenda.events.len());
        for (column, day) in (1_i16..).zip(&agenda.grid.days) {
            for (row, event_index) in (1_i16..).zip(day.events.clone()) {
                if let Some(option) =
                    self.render_week_event(agenda, event_index, EventPresentation::Bar, cx)
                {
                    cells.push(
                        div()
                            .col_start(column)
                            .row_start(row)
                            .min_w_0()
                            .p_1()
                            .child(option)
                            .into_any_element(),
                    );
                }
            }
        }
        self.render_event_listbox(
            |listbox| {
                listbox
                    .absolute()
                    .inset_0()
                    .grid()
                    .grid_cols(7)
                    .content_start()
                    .border_1()
                    .border_color(transparent_black())
            },
            cells,
            cx,
        )
    }
}

fn render_day_headers(grid: &WeekGrid, colors: GridColors) -> impl IntoElement {
    div()
        .flex()
        .flex_shrink_0()
        .px(px(1.))
        .border_b_1()
        .border_color(colors.border)
        .children(
            grid.days
                .iter()
                .enumerate()
                .map(|(index, day)| render_day_header(day, index, colors)),
        )
}

fn render_day_header(day: &DayColumn, index: usize, colors: GridColors) -> impl IntoElement {
    let date_id = day.date.strftime("%Y-%m-%d").to_string();
    let selector = format!("calendar-day-header-{date_id}");
    let number_selector = format!("calendar-day-number-{date_id}");
    let weekday_color = if day.is_today {
        colors.primary
    } else {
        colors.muted_foreground
    };
    let header = div()
        .id(selector.clone())
        .debug_selector(move || selector.clone())
        .role(Role::Group)
        .aria_label(day_header_label(day.date, day.is_today));
    #[cfg(test)]
    let header = header.test_support();
    header
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .gap_0p5()
        .py_2()
        .when(index > 0, |header| {
            header.border_l_1().border_color(colors.border)
        })
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(weekday_color)
                .child(weekday_abbreviation(day.date)),
        )
        .child(
            div()
                .debug_selector(move || number_selector.clone())
                .size_8()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_sm()
                .font_semibold()
                .when(day.is_today, |number| {
                    number
                        .bg(colors.primary)
                        .text_color(colors.primary_foreground)
                })
                .child(day.date.day().to_string()),
        )
}

/// The column dividers and today's tint, behind the events so that empty days still read as days.
fn render_column_backdrop(grid: &WeekGrid, colors: GridColors) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .px(px(1.))
        .children(grid.days.iter().enumerate().map(|(index, day)| {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .when(index > 0, |column| {
                    column.border_l_1().border_color(colors.border)
                })
                .when(day.is_today, |column| {
                    column.bg(colors.primary.opacity(TODAY_COLUMN_TINT_OPACITY))
                })
        }))
}
