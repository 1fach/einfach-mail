use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use einfach_mail_domain::{AccountId, CalendarViewReference};

/// Calendar write state for one view and its account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarWriteReadiness {
    /// This view has completed a sync, and no account view needs recovery.
    Ready,
    /// This view must sync successfully before it can accept an event write.
    ViewSyncRequired,
    /// An account view needs recovery or has an active replacement scan.
    AccountSyncRequired,
}

/// Recovery state shared by sessions for one account.
#[derive(Default)]
pub(crate) struct AccountWorkflowRecovery {
    mail_sync_ready: AtomicBool,
    calendar_write: Mutex<CalendarWriteRecovery>,
}

impl AccountWorkflowRecovery {
    pub(crate) fn mail_sync_ready(&self) -> bool {
        self.mail_sync_ready.load(Ordering::Acquire)
    }

    pub(crate) fn mark_mail_sync_required(&self) {
        self.mail_sync_ready.store(false, Ordering::Release);
    }

    pub(crate) fn mark_mail_sync_ready(&self) {
        self.mail_sync_ready.store(true, Ordering::Release);
    }

    pub(crate) fn calendar_write_readiness(
        &self,
        view: &CalendarViewReference,
    ) -> Result<CalendarWriteReadiness, RecoveryStateUnavailable> {
        self.calendar_write
            .lock()
            .map(|recovery| recovery.readiness_for(view))
            .map_err(|_| RecoveryStateUnavailable)
    }

    pub(crate) fn require_account_calendar_write_sync(
        &self,
        account_id: &AccountId,
    ) -> Result<(), RecoveryStateUnavailable> {
        self.calendar_write
            .lock()
            .map(|mut recovery| recovery.require_account_sync(account_id))
            .map_err(|_| RecoveryStateUnavailable)
    }

    pub(crate) fn mark_calendar_view_synchronized(
        &self,
        view: &CalendarViewReference,
    ) -> Result<(), RecoveryStateUnavailable> {
        self.calendar_write
            .lock()
            .map(|mut recovery| recovery.mark_view_synchronized(view))
            .map_err(|_| RecoveryStateUnavailable)
    }
}

#[derive(Default)]
struct CalendarWriteRecovery {
    synchronized_views: std::collections::HashSet<CalendarViewReference>,
    sync_required_views: std::collections::HashSet<CalendarViewReference>,
}

impl CalendarWriteRecovery {
    fn readiness_for(&self, view: &CalendarViewReference) -> CalendarWriteReadiness {
        if self
            .sync_required_views
            .iter()
            .any(|required_view| required_view.account_id() == view.account_id())
        {
            CalendarWriteReadiness::AccountSyncRequired
        } else if self.synchronized_views.contains(view) {
            CalendarWriteReadiness::Ready
        } else {
            CalendarWriteReadiness::ViewSyncRequired
        }
    }

    fn require_account_sync(&mut self, account_id: &AccountId) {
        self.sync_required_views.extend(
            self.synchronized_views
                .iter()
                .filter(|view| view.account_id() == account_id)
                .cloned(),
        );
    }

    fn mark_view_synchronized(&mut self, view: &CalendarViewReference) {
        self.synchronized_views.insert(view.clone());
        self.sync_required_views.remove(view);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryStateUnavailable;
