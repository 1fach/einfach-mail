use std::sync::Arc;

use einfach_mail_application::{CalendarSession, CalendarWorkflowError, CalendarWriteReadiness};
use einfach_mail_domain::CalendarViewReference;
use gpui_kit::{AsyncApp, Context, WeakEntity};

use super::CalendarView;
use super::state::CalendarStatus;

struct CalendarRefreshRequest<S: CalendarSession + ?Sized> {
    session: Arc<S>,
    view: CalendarViewReference,
    request_id: u64,
}

struct CalendarRefreshOutcome {
    sync_result: Result<einfach_mail_application::CalendarSyncReport, CalendarWorkflowError>,
    snapshot_result: Result<einfach_mail_application::CalendarSnapshot, CalendarWorkflowError>,
}

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.status.is_sync_active() {
            return;
        }
        self.status = CalendarStatus::LoadingCache;
        self.request_id = self.request_id.saturating_add(1);
        let request = CalendarRefreshRequest {
            session: Arc::clone(&self.session),
            view: self.week.view().clone(),
            request_id: self.request_id,
        };
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            Self::run_refresh(request, this, cx).await;
        }));
        cx.notify();
    }

    async fn run_refresh(
        request: CalendarRefreshRequest<S>,
        this: WeakEntity<Self>,
        cx: &mut AsyncApp,
    ) {
        let cached = request.session.load_cached_view(&request.view).await;
        if !Self::show_cached_snapshot(&this, cx, request.request_id, cached) {
            return;
        }
        let outcome = Self::load_refresh_outcome(&request).await;
        Self::finish_refresh(&this, cx, request.request_id, outcome);
    }

    async fn load_refresh_outcome(request: &CalendarRefreshRequest<S>) -> CalendarRefreshOutcome {
        let sync_result = request.session.synchronize_view(&request.view).await;
        let snapshot_result = request.session.load_cached_view(&request.view).await;
        CalendarRefreshOutcome {
            sync_result,
            snapshot_result,
        }
    }

    fn show_cached_snapshot(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        snapshot: Result<einfach_mail_application::CalendarSnapshot, CalendarWorkflowError>,
    ) -> bool {
        match snapshot {
            Ok(snapshot) => Self::show_saved_events(this, cx, request_id, snapshot),
            Err(error) => Self::show_cache_failure(this, cx, request_id, error),
        }
    }

    fn show_saved_events(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        snapshot: einfach_mail_application::CalendarSnapshot,
    ) -> bool {
        this.update(cx, |view, cx| {
            if view.request_id != request_id {
                return false;
            }
            view.apply_snapshot(snapshot);
            view.status = CalendarStatus::Syncing;
            cx.notify();
            true
        })
        .unwrap_or_default()
    }

    fn show_cache_failure(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        error: CalendarWorkflowError,
    ) -> bool {
        let _ = this.update(cx, |view, cx| {
            if view.request_id != request_id {
                return;
            }
            view.task = None;
            view.status = CalendarStatus::Failed(error);
            cx.notify();
        });
        false
    }

    fn finish_refresh(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        outcome: CalendarRefreshOutcome,
    ) {
        let _ = this.update(cx, |view, cx| {
            if view.request_id != request_id {
                return;
            }
            view.task = None;
            view.apply_refresh_outcome(outcome);
            cx.notify();
        });
    }

    fn apply_refresh_outcome(&mut self, outcome: CalendarRefreshOutcome) {
        match outcome.snapshot_result {
            Ok(snapshot) => {
                self.apply_snapshot(snapshot);
                self.status = match outcome.sync_result {
                    Ok(_) => CalendarStatus::UpToDate,
                    Err(error) => CalendarStatus::Failed(error),
                };
            }
            Err(error) => self.status = CalendarStatus::Failed(error),
        }
    }

    fn apply_snapshot(&mut self, snapshot: einfach_mail_application::CalendarSnapshot) {
        self.events = snapshot.events().to_vec();
        self.pending_creation = snapshot.pending_creation().cloned();
        self.write_readiness = snapshot.write_readiness();
        if let Some(selected) = self.selected_agenda_event.clone() {
            let agenda = self.week_agenda();
            let scroll_index = agenda
                .position_of(&selected)
                .map(|event_index| agenda.scroll_index(event_index, self.layout));
            if let Some(scroll_index) = scroll_index {
                self.agenda_scroll.scroll_to_item(scroll_index);
            } else {
                self.selected_agenda_event = None;
            }
        }
        self.sync_state = Some(snapshot.sync_state().clone());
    }

    pub(super) fn reload_cached_view(&mut self, cx: &mut Context<Self>) {
        if self.status.is_sync_active() {
            return;
        }
        self.request_id = self.request_id.saturating_add(1);
        let request_id = self.request_id;
        let session = Arc::clone(&self.session);
        let view = self.week.view().clone();
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let snapshot = session.load_cached_view(&view).await;
            Self::finish_cache_reload(&this, cx, request_id, snapshot);
        }));
    }

    fn finish_cache_reload(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        snapshot: Result<einfach_mail_application::CalendarSnapshot, CalendarWorkflowError>,
    ) {
        let _ = this.update(cx, |view, cx| {
            if view.request_id != request_id {
                return;
            }
            view.task = None;
            match snapshot {
                Ok(snapshot) => view.apply_snapshot(snapshot),
                Err(error) => view.status = CalendarStatus::Failed(error),
            }
            cx.notify();
        });
    }

    pub(super) fn require_calendar_sync_for_write(&mut self, cx: &mut Context<Self>) {
        self.write_readiness = CalendarWriteReadiness::AccountSyncRequired;
        cx.notify();
    }

    pub(crate) fn stop_pending_work(&mut self) {
        drop(self.task.take());
        self.request_id = self.request_id.saturating_add(1);
        self.status = CalendarStatus::Idle;
        self.events.clear();
        self.pending_creation = None;
        self.selected_agenda_event = None;
        self.sync_state = None;
        self.write_readiness = CalendarWriteReadiness::ViewSyncRequired;
    }
}
