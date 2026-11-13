use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::{Future, ready},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use einfach_mail_domain::{
    AccountId, CalendarCapabilities, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage,
    CalendarDeltaProgress, CalendarEventCreation, CalendarEventCreationId, CalendarEventUpdate,
    CalendarEventUpdateDraft, CalendarEventVersion, CalendarOccurrence, CalendarOccurrencePatch,
    CalendarOccurrenceTime, CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
    EventId, PendingCalendarEventCreation, VersionedCalendarEvent,
};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

use crate::{
    CalendarDeltaPageResponse, CalendarSyncMode, CalendarSyncStrategy,
    CalendarViewSnapshotResponse, shared::recovery::AccountWorkflowRecovery,
    shared::reservations::OperationReservations,
};

use crate::{
    CalendarCache, CalendarCacheError, CalendarDeltaRequest, CalendarDeltaResponse,
    CalendarEventCreationStage, CalendarOperationError, CalendarProvider, CalendarSyncState,
    CalendarWorkflow,
};

include!("test_support/calendar/provider.rs");
include!("test_support/calendar/cache.rs");
include!("test_support/calendar/builders.rs");
