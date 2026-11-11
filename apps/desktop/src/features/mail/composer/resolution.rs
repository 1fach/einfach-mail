use std::sync::Arc;

use einfach_mail_application::{MailCacheError, MailSendResolutionError};
use gpui_kit::{AsyncApp, Context};

use super::recovery::{PendingSendCertainty, PendingSendState};
use super::{ComposerStatus, MailComposer, MailComposerEvent, status_after_rejection};

impl MailComposer {
    pub(super) fn resolve_pending(&mut self, cx: &mut Context<Self>) {
        let Some(marker) = self.pending_send.marker() else {
            return;
        };
        if self.task.is_some() {
            return;
        }
        let workflow = Arc::clone(&self.workflow);
        let operation_id = marker.pending.operation_id();
        let certainty = marker.certainty;
        self.status = ComposerStatus::Sending;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = workflow.resolve_pending_send(operation_id).await;
            let _ = this.update(cx, |composer, cx| {
                composer.apply_resolution_result(result, certainty, cx);
            });
        }));
        cx.notify();
    }

    fn apply_resolution_result(
        &mut self,
        result: Result<(), MailSendResolutionError>,
        certainty: PendingSendCertainty,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        match result {
            Ok(()) => self.finish_pending_resolution(certainty, cx),
            Err(MailSendResolutionError::PendingResolutionRequired(pending)) => {
                self.pending_send
                    .set_marker(pending, PendingSendCertainty::Unknown);
                self.status = ComposerStatus::OutcomeUnknown;
            }
            Err(MailSendResolutionError::Cache(error)) => {
                self.status = ComposerStatus::CacheFailed(error);
            }
            Err(MailSendResolutionError::OperationInProgress) => {
                self.status = ComposerStatus::CacheFailed(MailCacheError::Unavailable);
            }
            Err(MailSendResolutionError::StateUnavailable) => {
                self.status = ComposerStatus::CacheUnavailable;
            }
        }
        cx.notify();
    }

    fn finish_pending_resolution(
        &mut self,
        certainty: PendingSendCertainty,
        cx: &mut Context<Self>,
    ) {
        self.pending_send = PendingSendState::Available(None);
        self.status = match certainty {
            PendingSendCertainty::Accepted => ComposerStatus::Accepted,
            PendingSendCertainty::Rejected(error) => status_after_rejection(error),
            PendingSendCertainty::Unknown => ComposerStatus::DuplicateConfirmation,
        };
        cx.emit(MailComposerEvent::PendingSendChanged(None));
    }

    pub(super) fn confirm_duplicate_risk(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, ComposerStatus::DuplicateConfirmation) {
            return;
        }
        self.status = ComposerStatus::Editing;
        cx.notify();
    }
}
