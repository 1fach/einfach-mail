use einfach_mail_application::CalendarWorkflowError;
use einfach_mail_domain::{CalendarOccurrence, EventId};
use jiff::civil;

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
    pub(super) time_label: String,
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

pub(super) enum AgendaRow<'a> {
    DayHeading(civil::Date),
    EmptyDay,
    Event(AgendaEvent<'a>),
}

#[derive(Clone, Copy)]
pub(super) struct AgendaOptionPosition {
    pub(super) event_position: usize,
    pub(super) event_count: usize,
    pub(super) selected: bool,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct EventSortKey {
    pub(super) kind: u8,
    pub(super) seconds: i64,
    pub(super) nanoseconds: i32,
    pub(super) original_index: usize,
}
