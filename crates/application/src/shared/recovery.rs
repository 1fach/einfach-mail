use std::{
    collections::HashSet,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use einfach_mail_domain::CalendarViewReference;

/// Calendar write readiness exposed with a cached view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarWriteReadiness {
    /// This view completed a sync in the current workflow factory.
    Ready,
    /// This view must sync successfully before it can accept an event write.
    ViewSyncRequired,
    /// A persisted account view needs recovery or has an active replacement scan.
    AccountSyncRequired,
}

/// Recovery state shared by sessions for one account.
#[derive(Default)]
pub(crate) struct AccountWorkflowRecovery {
    mail_sync_ready: AtomicBool,
    calendar_views_synced: Mutex<HashSet<CalendarViewReference>>,
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
        self.calendar_views_synced
            .lock()
            .map(|views| {
                if views.contains(view) {
                    CalendarWriteReadiness::Ready
                } else {
                    CalendarWriteReadiness::ViewSyncRequired
                }
            })
            .map_err(|_| RecoveryStateUnavailable)
    }

    pub(crate) fn mark_calendar_view_synchronized(
        &self,
        view: &CalendarViewReference,
    ) -> Result<(), RecoveryStateUnavailable> {
        self.calendar_views_synced
            .lock()
            .map(|mut views| {
                views.insert(view.clone());
            })
            .map_err(|_| RecoveryStateUnavailable)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryStateUnavailable;
