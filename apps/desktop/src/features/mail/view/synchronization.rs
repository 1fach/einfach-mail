use crate::shared::relative_age_label;
use einfach_mail_application::{MailSyncState, MailWorkflowError};

use super::{MailView, mail_error_message};

pub(super) enum SyncStatus {
    Idle,
    LoadingCache,
    Syncing,
    Rebuilding,
    UpToDate,
    Failed(MailWorkflowError),
}

impl SyncStatus {
    pub(super) fn is_sync_active(&self) -> bool {
        match self {
            Self::LoadingCache | Self::Syncing | Self::Rebuilding => true,
            Self::Idle | Self::UpToDate | Self::Failed(_) => false,
        }
    }
}

impl MailView {
    pub(super) fn sync_status_label(&self) -> String {
        match &self.sync_status {
            SyncStatus::Idle => "Mail is ready to sync".to_owned(),
            SyncStatus::LoadingCache => "Opening encrypted cache…".to_owned(),
            SyncStatus::Syncing => {
                let label = if self
                    .sync_state
                    .as_ref()
                    .is_some_and(MailSyncState::is_rebuilding)
                {
                    "Rebuilding mail"
                } else {
                    "Syncing"
                };
                self.sync_age_label(label)
            }
            SyncStatus::Rebuilding => self.sync_age_label("Rebuilding mail"),
            SyncStatus::UpToDate => self.sync_age_label("Up to date"),
            SyncStatus::Failed(error) => {
                let rebuild_resume = self
                    .sync_state
                    .as_ref()
                    .is_some_and(MailSyncState::is_rebuilding)
                    && *error != MailWorkflowError::RepeatedDeltaReset;
                let cached_age = self.sync_state.as_ref().and_then(|state| {
                    state.last_synchronized_at().map(|last_sync| {
                        format!(" Cached mail from {}.", relative_age_label(last_sync))
                    })
                });
                format!(
                    "{}{}{}",
                    mail_error_message(*error),
                    cached_age.unwrap_or_default(),
                    if rebuild_resume {
                        " The saved mail rebuild is incomplete. Choose Sync to resume it."
                    } else {
                        ""
                    }
                )
            }
        }
    }

    fn sync_age_label(&self, prefix: &str) -> String {
        self.sync_state
            .as_ref()
            .and_then(MailSyncState::last_synchronized_at)
            .map_or_else(
                || prefix.to_owned(),
                |time| format!("{prefix} · {}", relative_age_label(time)),
            )
    }
}
