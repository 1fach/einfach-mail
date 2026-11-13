use std::ops::Range;

use einfach_mail_application::CalendarWorkflowError;
use einfach_mail_domain::{CalendarOccurrence, EventId};
use gpui_kit::{Pixels, Rems, rems};
use jiff::civil;

use crate::components::NAVIGATION_PANE_WIDTH;

#[derive(Clone, Copy)]
pub(super) enum CalendarStatus {
    Idle,
    LoadingCache,
    Syncing,
    UpToDate,
    Failed(CalendarWorkflowError),
}

impl CalendarStatus {
    pub(super) fn is_sync_active(&self) -> bool {
        match self {
            Self::LoadingCache | Self::Syncing => true,
            Self::Idle | Self::UpToDate | Self::Failed(_) => false,
        }
    }
}

pub(super) struct DisplayEvent<'a> {
    pub(super) event: &'a CalendarOccurrence,
    pub(super) sort_key: EventSortKey,
    /// The full time range with its zone, for the compact agenda and the accessible name.
    pub(super) time_label: String,
    /// The time range without a zone, for an event bar that has little width.
    pub(super) bar_time_label: String,
}

#[derive(Clone, Eq, PartialEq)]
pub(super) struct AgendaEventKey {
    pub(super) date: civil::Date,
    pub(super) event_id: EventId,
}

pub(super) struct AgendaEvent<'a> {
    pub(super) key: AgendaEventKey,
    pub(super) display: DisplayEvent<'a>,
}

/// One day of the week: its date and which events of the flat order fall on it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DayColumn {
    pub(super) date: civil::Date,
    pub(super) is_today: bool,
    /// Indices into [`WeekAgenda::events`]. Events are ordered day by day, so the events of one
    /// day are a contiguous range.
    pub(super) events: Range<usize>,
}

/// The seven days of the week, Monday first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WeekGrid {
    pub(super) days: [DayColumn; 7],
}

/// The week's events in the one order that the keyboard, the screen reader and both layouts
/// share: day by day, then by time. The grid only arranges that order.
pub(super) struct WeekAgenda<'a> {
    pub(super) events: Vec<AgendaEvent<'a>>,
    pub(super) grid: WeekGrid,
}

/// How the week is laid out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CalendarLayout {
    /// Seven day columns side by side.
    Grid,
    /// One column of day headings and events.
    Agenda,
}

/// The narrowest width beside the navigation pane that fits seven readable day columns. Below it
/// the week reads as an agenda.
const GRID_MINIMUM_WIDTH: Rems = rems(44.);

impl CalendarLayout {
    /// The layout for a window `window_width` wide. The navigation pane takes its fixed width
    /// first, and the calendar gets the rest.
    pub(super) fn for_window_width(window_width: Pixels, rem_size: Pixels) -> Self {
        let content_width = window_width - NAVIGATION_PANE_WIDTH.to_pixels(rem_size);
        if content_width >= GRID_MINIMUM_WIDTH.to_pixels(rem_size) {
            Self::Grid
        } else {
            Self::Agenda
        }
    }
}

impl WeekAgenda<'_> {
    /// The position of the event identified by `key` in the flat order.
    pub(super) fn position_of(&self, key: &AgendaEventKey) -> Option<usize> {
        self.events.iter().position(|event| &event.key == key)
    }

    /// The index, among the children of the scrolling listbox, of the child that holds the event
    /// at `event_index` in `layout`.
    pub(super) fn scroll_index(&self, event_index: usize, layout: CalendarLayout) -> usize {
        match layout {
            CalendarLayout::Grid => event_index,
            CalendarLayout::Agenda => {
                let mut children_before = 0;
                for day in &self.grid.days {
                    if day.events.contains(&event_index) {
                        return children_before + 1 + (event_index - day.events.start);
                    }
                    children_before += 1 + day.events.len().max(1);
                }
                children_before
            }
        }
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct EventSortKey {
    pub(super) kind: u8,
    pub(super) seconds: i64,
    pub(super) nanoseconds: i32,
    pub(super) original_index: usize,
}
