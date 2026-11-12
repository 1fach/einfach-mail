use std::{
    future::{Future, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use einfach_mail_application::{
    CalendarDeltaPageResponse, CalendarDeltaRequest, CalendarDeltaResponse, CalendarOperationError,
    CalendarProvider, CalendarSyncStrategy,
};
use einfach_mail_domain::{
    CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress, CalendarEventCreation,
    CalendarEventUpdate, CalendarEventVersion, CalendarOccurrence, CalendarOccurrencePatch,
    CalendarOccurrenceTime, CalendarOccurrenceTimePatch, CalendarViewReference, EventId,
    VersionedCalendarEvent,
};
use tokio::sync::oneshot;

#[derive(Clone, Default)]
pub(crate) struct TestProvider {
    pub(crate) requests: Arc<Mutex<Vec<(i64, i64)>>>,
    calendar_sync_succeeds: Arc<AtomicBool>,
    calendar_sync_failure: Arc<Mutex<Option<CalendarOperationError>>>,
    event_creation_gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    event_creation_failure: Arc<Mutex<Option<CalendarOperationError>>>,
    pub(crate) event_creation_count: Arc<AtomicUsize>,
    pub(crate) event_update_calls: Arc<AtomicUsize>,
    event_update_gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    event_update_failure: Arc<Mutex<Option<CalendarOperationError>>>,
    pub(crate) event_deletion_calls: Arc<AtomicUsize>,
    event_deletion_gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    event_deletion_failure: Arc<Mutex<Option<CalendarOperationError>>>,
}

impl TestProvider {
    pub(crate) fn allow_calendar_sync(&self) {
        self.calendar_sync_succeeds.store(true, Ordering::Release);
    }

    pub(crate) fn fail_next_calendar_sync(&self, failure: CalendarOperationError) {
        *self
            .calendar_sync_failure
            .lock()
            .expect("synthetic calendar sync failure mutex is not poisoned") = Some(failure);
    }

    pub(crate) fn hold_event_update_until(&self, gate: oneshot::Receiver<()>) {
        *self
            .event_update_gate
            .lock()
            .expect("synthetic update gate mutex is not poisoned") = Some(gate);
    }

    pub(crate) fn fail_next_event_update(&self, failure: CalendarOperationError) {
        *self
            .event_update_failure
            .lock()
            .expect("synthetic update failure mutex is not poisoned") = Some(failure);
    }

    pub(crate) fn hold_event_deletion_until(&self, gate: oneshot::Receiver<()>) {
        *self
            .event_deletion_gate
            .lock()
            .expect("synthetic deletion gate mutex is not poisoned") = Some(gate);
    }

    pub(crate) fn fail_next_event_deletion(&self, failure: CalendarOperationError) {
        *self
            .event_deletion_failure
            .lock()
            .expect("synthetic deletion failure mutex is not poisoned") = Some(failure);
    }

    pub(crate) fn hold_event_creation_until(&self, gate: oneshot::Receiver<()>) {
        *self
            .event_creation_gate
            .lock()
            .expect("synthetic creation gate mutex is not poisoned") = Some(gate);
    }

    pub(crate) fn fail_next_event_creation(&self, failure: CalendarOperationError) {
        *self
            .event_creation_failure
            .lock()
            .expect("synthetic creation failure mutex is not poisoned") = Some(failure);
    }

    fn take_event_creation_gate(&self) -> Option<oneshot::Receiver<()>> {
        self.event_creation_gate
            .lock()
            .expect("synthetic creation gate mutex is not poisoned")
            .take()
    }

    fn take_event_creation_failure(&self) -> Option<CalendarOperationError> {
        self.event_creation_failure
            .lock()
            .expect("synthetic creation failure mutex is not poisoned")
            .take()
    }
}

impl CalendarProvider for TestProvider {
    fn calendar_sync_strategy(
        &self,
    ) -> impl Future<Output = Result<CalendarSyncStrategy, CalendarOperationError>> + Send + '_
    {
        ready(Ok(CalendarSyncStrategy::Delta))
    }

    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        _: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        self.requests
            .lock()
            .expect("synthetic calendar request log mutex is not poisoned")
            .push((
                view.range().start().unix_timestamp(),
                view.range().end().unix_timestamp(),
            ));
        let failure = self
            .calendar_sync_failure
            .lock()
            .expect("synthetic calendar sync failure mutex is not poisoned")
            .take();
        let result = if let Some(failure) = failure {
            Err(failure)
        } else if self.calendar_sync_succeeds.load(Ordering::Acquire) {
            Ok(CalendarDeltaResponse::Page(CalendarDeltaPageResponse::new(
                CalendarDeltaPage::new(
                    Vec::new(),
                    CalendarDeltaProgress::Complete(CalendarDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken=synthetic-ui"
                            .to_owned(),
                    )),
                ),
                0,
            )))
        } else {
            Err(CalendarOperationError::AccessDenied)
        };
        ready(result)
    }

    fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let gate = self.take_event_creation_gate();
        let failure = self.take_event_creation_failure();
        let creation = creation.clone();
        let event_creation_count = Arc::clone(&self.event_creation_count);
        async move {
            event_creation_count.fetch_add(1, Ordering::SeqCst);
            create_event_after_wait(creation, gate, failure).await
        }
    }

    fn recover_event_creation(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        self.create_event(creation)
    }

    fn load_versioned_event(
        &self,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<VersionedCalendarEvent, CalendarOperationError>> + Send + '_
    {
        let event = event.clone();
        async move {
            VersionedCalendarEvent::new(
                event,
                CalendarEventVersion::try_from("synthetic-version-1".to_owned())
                    .map_err(|_| CalendarOperationError::InvalidResponse)?,
            )
            .map_err(|_| CalendarOperationError::EventNotEditable)
        }
    }

    fn update_event(
        &self,
        update: &CalendarEventUpdate,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let update = update.clone();
        let gate = self
            .event_update_gate
            .lock()
            .expect("synthetic update gate mutex is not poisoned")
            .take();
        let failure = self
            .event_update_failure
            .lock()
            .expect("synthetic update failure mutex is not poisoned")
            .take();
        let event_update_calls = Arc::clone(&self.event_update_calls);
        async move {
            event_update_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = gate {
                gate.await
                    .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
            }
            if let Some(failure) = failure {
                return Err(failure);
            }
            let mut event = update.source().occurrence().clone();
            let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
                return Err(CalendarOperationError::InvalidResponse);
            };
            let mut patch = CalendarOccurrencePatch::new();
            patch.set_subject(Some(update.draft().subject().to_owned()));
            let mut time = CalendarOccurrenceTimePatch::new();
            time.set_start(*start);
            time.set_end(*end);
            time.set_is_all_day(false);
            patch.set_time(time);
            patch
                .apply_to(&mut event)
                .map_err(|_| CalendarOperationError::InvalidResponse)?;
            Ok(event)
        }
    }

    fn delete_event(
        &self,
        _: &VersionedCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        let gate = self
            .event_deletion_gate
            .lock()
            .expect("synthetic deletion gate mutex is not poisoned")
            .take();
        let failure = self
            .event_deletion_failure
            .lock()
            .expect("synthetic deletion failure mutex is not poisoned")
            .take();
        let event_deletion_calls = Arc::clone(&self.event_deletion_calls);
        async move {
            event_deletion_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = gate {
                gate.await
                    .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
            }
            failure.map_or(Ok(()), Err)
        }
    }
}

async fn create_event_after_wait(
    creation: CalendarEventCreation,
    gate: Option<oneshot::Receiver<()>>,
    failure: Option<CalendarOperationError>,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    if let Some(gate) = gate {
        gate.await
            .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
    }
    match failure {
        Some(failure) => Err(failure),
        None => created_event(&creation),
    }
}

fn created_event(
    creation: &CalendarEventCreation,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        return Err(CalendarOperationError::InvalidResponse);
    };
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(creation.draft().subject().to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(*start);
    time.set_end(*end);
    time.set_is_all_day(false);
    patch.set_time(time);
    let id = EventId::try_from(format!(
        "synthetic-created-event-{}",
        creation.id().as_uuid()
    ))
    .map_err(|_| CalendarOperationError::InvalidResponse)?;
    patch
        .into_new(creation.view().clone(), id)
        .map_err(|_| CalendarOperationError::InvalidResponse)
}
