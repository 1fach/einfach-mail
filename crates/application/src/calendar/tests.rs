use std::{sync::Arc, time::UNIX_EPOCH};

use einfach_mail_domain::{
    CalendarDeltaChange, CalendarDeltaProgress, CalendarEventCreation, CalendarEventUpdate,
    CalendarEventUpdateDraft, CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, EventId, PendingCalendarEventCreation,
};
use time::{Date, Month};

use crate::{CalendarViewSnapshotResponse, shared::sync_budget::MAX_SYNC_RESPONSE_BYTES};

use crate::{
    CalendarCache, CalendarCacheError, CalendarDeltaRequest, CalendarDeltaResponse,
    CalendarEventCreationOutcome, CalendarEventDeletionOutcome, CalendarEventUpdateOutcome,
    CalendarOperationError, CalendarWorkflow, CalendarWorkflowError, CalendarWriteReadiness,
};

use crate::test_support::{
    FixtureCache, FixtureProvider, all_day_patch, creation, cross_week_timed_event_patch, event_id,
    link, replacement_event_id, response, synchronize_empty_view, update_request, utc, view,
    view_for_week, workflow_account_id, workflows_sharing_account_state,
};

// Keep `calendar::tests::<name>` stable while storing each behavior group in its own file.
include!("tests/sync.rs");
include!("tests/replacement_recovery.rs");
include!("tests/creation.rs");
include!("tests/update_recovery.rs");
include!("tests/delete_recovery.rs");
