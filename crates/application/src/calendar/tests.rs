use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use einfach_mail_domain::{
    AccountId, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference, EventId,
};
use time::{Date, Month, OffsetDateTime};

use super::{
    CalendarCache, CalendarCacheError, CalendarDeltaPageResponse, CalendarDeltaRequest,
    CalendarOperationError, CalendarProvider, CalendarSyncState, CalendarWorkflow,
    CalendarWorkflowError,
};

#[derive(Clone, Default)]
struct FixtureProvider {
    pages: Arc<Mutex<VecDeque<Result<CalendarDeltaPageResponse, CalendarOperationError>>>>,
    requests: Arc<Mutex<Vec<CalendarDeltaRequest>>>,
}

impl FixtureProvider {
    fn with_pages(
        pages: impl IntoIterator<Item = Result<CalendarDeltaPageResponse, CalendarOperationError>>,
    ) -> Self {
        Self {
            pages: Arc::new(Mutex::new(pages.into_iter().collect())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn push_page(&self, page: Result<CalendarDeltaPageResponse, CalendarOperationError>) {
        self.pages
            .lock()
            .expect("synthetic page queue mutex is not poisoned")
            .push_back(page);
    }
}

impl CalendarProvider for FixtureProvider {
    fn calendar_delta_page(
        &self,
        _view: &CalendarViewReference,
        request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaPageResponse, CalendarOperationError>> + Send + '_
    {
        let pages = Arc::clone(&self.pages);
        let requests = Arc::clone(&self.requests);
        async move {
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .push(request);
            pages
                .lock()
                .expect("synthetic page queue mutex is not poisoned")
                .pop_front()
                .unwrap_or(Err(CalendarOperationError::ServiceUnavailable))
        }
    }
}

#[derive(Clone, Default)]
struct FixtureCache {
    views: Arc<Mutex<HashMap<CalendarViewReference, CachedView>>>,
}

#[derive(Clone)]
struct CachedView {
    state: CalendarSyncState,
    events: HashMap<String, CalendarOccurrence>,
}

impl CachedView {
    fn empty() -> Self {
        Self {
            state: CalendarSyncState::initial(),
            events: HashMap::new(),
        }
    }
}

impl CalendarCache for FixtureCache {
    fn ensure_view(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let view = view.clone();
        async move {
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            Ok(views
                .entry(view)
                .or_insert_with(CachedView::empty)
                .state
                .clone())
        }
    }

    fn commit_delta_page(
        &self,
        view: &CalendarViewReference,
        page: CalendarDeltaPage,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let view = view.clone();
        async move {
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            let cached = views.entry(view.clone()).or_insert_with(CachedView::empty);
            let mut staged = cached.clone();
            let (changes, progress) = page.into_parts();
            for change in changes {
                match change {
                    CalendarDeltaChange::Removed { id } => {
                        staged.events.remove(id.as_str());
                    }
                    CalendarDeltaChange::Upsert { id, patch } => {
                        let event = match staged.events.remove(id.as_str()) {
                            Some(mut event) => {
                                patch
                                    .apply_to(&mut event)
                                    .map_err(|_| CalendarCacheError::InvalidData)?;
                                event
                            }
                            None => patch
                                .into_new(view.clone(), id.clone())
                                .map_err(|_| CalendarCacheError::InvalidData)?,
                        };
                        if event.overlaps_view() {
                            staged.events.insert(id.as_str().to_owned(), event);
                        }
                    }
                }
            }
            staged.state = match progress {
                CalendarDeltaProgress::Next(link) => CalendarSyncState::from_checkpoint(
                    staged.state.delta_link().cloned(),
                    Some(link),
                    staged.state.last_synchronized_at(),
                )?,
                CalendarDeltaProgress::Complete(link) => {
                    CalendarSyncState::from_checkpoint(Some(link), None, Some(completed_at))?
                }
            };
            let state = staged.state.clone();
            *cached = staged;
            Ok(state)
        }
    }

    fn list_events(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<Vec<CalendarOccurrence>, CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let view = view.clone();
        async move {
            let views = views.lock().expect("synthetic cache mutex is not poisoned");
            Ok(views
                .get(&view)
                .map(|cached| cached.events.values().cloned().collect())
                .unwrap_or_default())
        }
    }

    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
        let views = Arc::clone(&self.views);
        let account_id = account_id.clone();
        async move {
            let mut views = views.lock().expect("synthetic cache mutex is not poisoned");
            if views.keys().any(|view| view.account_id() != &account_id) {
                return Err(CalendarCacheError::AccountMismatch);
            }
            views.retain(|view, _| view.account_id() != &account_id);
            Ok(())
        }
    }
}

fn view() -> CalendarViewReference {
    let account = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account identifier is valid");
    let start_date = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic view start date is valid");
    let end_date = Date::from_calendar_date(2030, Month::January, 14)
        .expect("synthetic view end date is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic week range is valid");
    CalendarViewReference::new(account, range)
}

fn event_id() -> EventId {
    EventId::try_from("synthetic-event".to_owned()).expect("synthetic event ID is valid")
}

fn all_day_patch(subject: Option<&str>, location: Option<&str>) -> CalendarOccurrencePatch {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(subject.map(str::to_owned));
    patch.set_location(location.map(str::to_owned));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(utc(2030, Month::January, 8));
    time.set_end(utc(2030, Month::January, 10));
    time.set_is_all_day(true);
    patch.set_time(time);
    patch
}

fn utc(year: i32, month: Month, day: u8) -> OffsetDateTime {
    Date::from_calendar_date(year, month, day)
        .expect("synthetic date is valid")
        .midnight()
        .assume_utc()
}

fn link(cursor_token: &str) -> CalendarDeltaLink {
    CalendarDeltaLink::from_opaque(format!(
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken={cursor_token}"
    ))
}

fn response(
    changes: Vec<CalendarDeltaChange>,
    progress: CalendarDeltaProgress,
) -> CalendarDeltaPageResponse {
    CalendarDeltaPageResponse::new(CalendarDeltaPage::new(changes, progress), 128)
}

#[test]
fn sync_commits_pages_and_preserves_omitted_fields() {
    smol::block_on(async {
        let view = view();
        let next_link = link("synthetic-next");
        let delta_link = link("synthetic-complete");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: all_day_patch(Some("Synthetic appointment"), Some("Room 1")),
                }],
                CalendarDeltaProgress::Next(next_link.clone()),
            )),
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: {
                        let mut patch = CalendarOccurrencePatch::new();
                        patch.set_location(None);
                        let mut time = CalendarOccurrenceTimePatch::new();
                        time.set_start(utc(2030, Month::January, 9));
                        patch.set_time(time);
                        patch
                    },
                }],
                CalendarDeltaProgress::Complete(delta_link.clone()),
            )),
        ]);
        let request_log = Arc::clone(&provider.requests);
        let workflow = CalendarWorkflow::new(provider, FixtureCache::default());

        let report = workflow.sync_view(&view).await.expect("both pages commit");
        assert_eq!(report.page_count(), 2);
        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(next_link)
            ]
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic snapshot is readable");
        assert_eq!(snapshot.sync_state().delta_link(), Some(&delta_link));
        assert!(snapshot.sync_state().next_link().is_none());
        assert_eq!(snapshot.events().len(), 1);
        assert_eq!(
            snapshot.events()[0].subject(),
            Some("Synthetic appointment")
        );
        assert_eq!(snapshot.events()[0].location(), None);
        assert!(matches!(
            snapshot.events()[0].time(),
            CalendarOccurrenceTime::AllDay { start, end_exclusive }
                if *start == Date::from_calendar_date(2030, Month::January, 9).expect("valid date")
                    && *end_exclusive == Date::from_calendar_date(2030, Month::January, 10).expect("valid date")
        ));
    });
}

#[test]
fn interrupted_sync_resumes_the_saved_next_link() {
    smol::block_on(async {
        let view = view();
        let next_link = link("synthetic-resume");
        let delta_link = link("synthetic-final");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: all_day_patch(Some("Synthetic appointment"), None),
                }],
                CalendarDeltaProgress::Next(next_link.clone()),
            )),
            Err(CalendarOperationError::ServiceUnavailable),
        ]);
        let request_log = Arc::clone(&provider.requests);
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(provider.clone(), cache);

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(delta_link.clone()),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("the interrupted cursor resumes");

        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(next_link.clone()),
                CalendarDeltaRequest::Continue(next_link),
            ]
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic snapshot is readable");
        assert_eq!(snapshot.sync_state().delta_link(), Some(&delta_link));
        assert!(snapshot.sync_state().next_link().is_none());
        assert_eq!(snapshot.events().len(), 1);
    });
}

#[test]
fn repeated_delta_cursor_stops_before_committing_the_looping_page() {
    smol::block_on(async {
        let view = view();
        let repeated_link = link("synthetic-repeated");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                Vec::new(),
                CalendarDeltaProgress::Next(repeated_link.clone()),
            )),
            Ok(response(
                Vec::new(),
                CalendarDeltaProgress::Next(repeated_link),
            )),
        ]);
        let workflow = CalendarWorkflow::new(provider, FixtureCache::default());

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::RepeatedCursor)
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic cursor state is readable");
        assert!(snapshot.sync_state().next_link().is_some());
    });
}

#[test]
fn a_completed_cursor_remains_available_for_the_next_round() {
    smol::block_on(async {
        let view = view();
        let delta_link = link("synthetic-delta");
        let provider = FixtureProvider::with_pages([Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(delta_link.clone()),
        ))]);
        let workflow = CalendarWorkflow::new(provider.clone(), FixtureCache::default());

        workflow
            .sync_view(&view)
            .await
            .expect("initial round completes");
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("synthetic-next-round")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("following round completes");

        assert_eq!(
            *provider
                .requests
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(delta_link)
            ]
        );
    });
}
