use super::week::CalendarWeek;
use std::sync::Arc;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike, Weekday};
use einfach_mail_application::{
    CalendarCacheError, CalendarEventCreationOutcome, CalendarEventDeletionOutcome,
    CalendarEventUpdateOutcome, CalendarOperationError, CalendarSession, CalendarWorkflowError,
};
use einfach_mail_domain::{
    CalendarAction, CalendarCapabilities, CalendarEventCreation, CalendarEventDraft,
    CalendarEventUpdate, CalendarEventUpdateDraft, CalendarOccurrence, CalendarOccurrenceTime,
    CalendarTimeZoneId, CalendarViewRange, PendingCalendarEventCreation, VersionedCalendarEvent,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    date_picker::{DatePicker, DatePickerState},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputState},
    time_field::TimePrecision,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, Entity, EventEmitter, IntoElement, KeyBinding, Render,
    Role, Task, Window, div, prelude::*,
};
use jiff::{civil, tz::TimeZone};

const EVENT_COMPOSER_WRITE_PENDING_CONTEXT: &str = "CalendarEventWritePending";

pub(crate) fn bind_event_composer_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        gpui_kit::NoAction,
        Some(EVENT_COMPOSER_WRITE_PENDING_CONTEXT),
    )]);
}

/// Reports composer outcomes and requests for calendar-view actions.
pub(crate) enum EventComposerEvent {
    /// Reloads cached occurrences after an event write.
    CalendarChanged,
    /// Blocks further event writes until a successful sync reconciles an uncertain outcome.
    CalendarWriteSyncRequired,
    /// Reloads the saved creation operation after a create attempt or cleanup.
    PendingOperationUpdated,
    /// Opens the account's calendar write permission review.
    ReviewCalendarWritePermission,
}

/// Selects a new event, a saved creation retry, or an existing event to load.
pub(super) enum EventComposerIntent {
    /// Creates an event or resumes the account's saved creation.
    Create {
        /// A persisted operation to retry with its existing id, when present.
        pending_creation: Option<PendingCalendarEventCreation>,
    },
    /// Loads a fresh provider snapshot before showing or changing the event.
    Existing {
        /// The cached occurrence used to locate the current provider event.
        cached_event: CalendarOccurrence,
    },
}

/// Provides the account capabilities, view, and local-time context for the composer.
pub(crate) struct EventComposerSetup<S: CalendarSession + ?Sized + 'static> {
    /// Account-scoped calendar session for loading and writing the event.
    pub(super) session: Arc<S>,
    pub(super) week: CalendarWeek,
    /// Calendar operations supported by the account's provider.
    pub(super) capabilities: CalendarCapabilities,
    pub(super) intent: EventComposerIntent,
}

/// Shows a timed event and performs the writes supported by its provider.
pub(crate) struct EventComposer<S: CalendarSession + ?Sized + 'static> {
    session: Arc<S>,
    week: CalendarWeek,
    capabilities: CalendarCapabilities,
    subject: Entity<InputState>,
    start: Entity<DatePickerState>,
    end: Entity<DatePickerState>,
    state: ComposerState,
    task: Option<Task<()>>,
}

use state::{ComposerState, CreateState, EditState};

impl<S: CalendarSession + ?Sized + 'static> EventComposer<S> {
    pub(super) fn can_submit(&self) -> bool {
        self.capabilities.supports(self.required_write_action()) && self.state.can_submit()
    }

    pub(super) fn should_render_fields(&self) -> bool {
        self.capabilities.supports(self.required_write_action()) && self.state.show_fields()
    }

    pub(super) fn can_delete(&self) -> bool {
        self.capabilities.supports(CalendarAction::DeleteEvent) && self.state.can_delete()
    }

    fn required_write_action(&self) -> CalendarAction {
        match &self.state {
            ComposerState::Create(_) => CalendarAction::CreateEvent,
            ComposerState::Edit(_) => CalendarAction::UpdateEvent,
        }
    }

    /// Creates a sheet for event creation, recovery, or an existing event.
    pub(crate) fn new(
        setup: EventComposerSetup<S>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let EventComposerSetup {
            session,
            week,
            capabilities,
            intent,
        } = setup;
        let (state, cached_event) = match intent {
            EventComposerIntent::Create { pending_creation } => (
                ComposerState::Create(CreateState::from_pending_creation(pending_creation)),
                None,
            ),
            EventComposerIntent::Existing { cached_event } => {
                (ComposerState::Edit(EditState::Loading), Some(cached_event))
            }
        };
        let (subject, start, end) = render::new_composer_inputs(week.start(), window, cx);
        let mut composer = Self {
            session,
            week,
            capabilities,
            subject,
            start,
            end,
            state,
            task: None,
        };
        if let Some(cached_event) = cached_event {
            composer.start_edit_load(cached_event, window, cx);
        }
        composer
    }
}

impl<S: CalendarSession + ?Sized + 'static> EventEmitter<EventComposerEvent> for EventComposer<S> {}

mod actions;
mod copy;
mod render;
mod state;
mod time;

#[cfg(test)]
mod tests;
