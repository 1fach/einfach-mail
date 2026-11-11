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

use self::support::{system_calendar_week, utc_fallback_calendar_week};
use super::{
    AgendaEventKey, AgendaRow, CalendarStatus, CalendarView, CalendarViewEvent,
    bind_calendar_event_navigation_keys, display_events,
};
use crate::test_support::calendar::{
    TestCache, TestProvider, agenda_events, draw_calendar_view, profile,
    read_only_calendar_profile, select_default_timed_event, timed_event, timed_event_with_id,
    workflow_account_id,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

#[cfg(test)]
mod agenda_display;
#[cfg(test)]
mod agenda_navigation;
#[cfg(test)]
mod composer;
#[cfg(test)]
mod support;
#[cfg(test)]
mod synchronization;
