use std::{sync::Arc, time::SystemTime};

use einfach_mail_application::{
    MailCacheError, MailSendOutcome, MailSendProviderError, MailSendWorkflowError, MailSession,
};
use einfach_mail_domain::PendingMailSend;
use gpui_kit::{AsyncApp, Context};

use super::recovery::PendingSendCertainty;
use super::{ComposerStatus, ComposerSubmission, MailComposer, MailComposerEvent};

async fn submit_composer_message(
    workflow: &dyn MailSession,
    submission: ComposerSubmission,
) -> Result<MailSendOutcome, MailSendWorkflowError> {
    match submission {
        ComposerSubmission::NewMessage(draft) => {
            workflow.send_message(draft, SystemTime::now()).await
        }
        ComposerSubmission::Reply(draft) => {
            workflow.reply_to_message(draft, SystemTime::now()).await
        }
    }
}

impl MailComposer {
    pub(super) fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_send() {
            return;
        }
        let submission = match self.build_submission(cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.status = ComposerStatus::InvalidDraft(error);
                cx.notify();
                return;
            }
        };
        self.start_submission(submission, cx);
    }

    fn start_submission(&mut self, submission: ComposerSubmission, cx: &mut Context<Self>) {
        let workflow = Arc::clone(&self.workflow);
        self.status = ComposerStatus::Sending;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let submission_result = submit_composer_message(workflow.as_ref(), submission).await;
            let pending_result = workflow.load_pending_send().await;
            let _ = this.update(cx, |composer, cx| {
                composer.apply_send_result(submission_result, pending_result, cx);
            });
        }));
        cx.notify();
    }

    fn apply_send_result(
        &mut self,
        result: Result<MailSendOutcome, MailSendWorkflowError>,
        pending_result: Result<Option<PendingMailSend>, MailCacheError>,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        let workflow_result_survives_missing_snapshot = !matches!(
            &result,
            Err(MailSendWorkflowError::Cache(_) | MailSendWorkflowError::StateUnavailable)
        );
        self.apply_pending_snapshot(pending_result);
        if self.pending_send.is_available() || workflow_result_survives_missing_snapshot {
            self.apply_send_workflow_result(result);
        }
        cx.emit(MailComposerEvent::PendingSendChanged(
            self.pending_send.pending().cloned(),
        ));
        cx.notify();
    }

    fn apply_pending_snapshot(
        &mut self,
        pending_result: Result<Option<PendingMailSend>, MailCacheError>,
    ) {
        self.pending_send.apply_snapshot(pending_result);
        if !self.pending_send.is_available() {
            self.status = ComposerStatus::CacheUnavailable;
        }
    }

    fn apply_send_workflow_result(
        &mut self,
        result: Result<MailSendOutcome, MailSendWorkflowError>,
    ) {
        self.status = match result {
            Ok(MailSendOutcome::Accepted) => {
                self.pending_send.clear_marker();
                ComposerStatus::Accepted
            }
            Ok(MailSendOutcome::AcceptedNeedsResolution(pending)) => {
                self.pending_send
                    .set_marker(pending, PendingSendCertainty::Accepted);
                ComposerStatus::AcceptedNeedsResolution
            }
            Ok(MailSendOutcome::OutcomeUnknown(pending))
            | Err(MailSendWorkflowError::PendingResolutionRequired(pending)) => {
                self.pending_send
                    .set_marker(pending, PendingSendCertainty::Unknown);
                ComposerStatus::OutcomeUnknown
            }
            Err(MailSendWorkflowError::RejectedNeedsResolution {
                pending,
                provider,
                cache,
            }) => {
                self.pending_send
                    .set_marker(*pending, PendingSendCertainty::Rejected(provider));
                ComposerStatus::RejectedNeedsResolution(cache)
            }
            Err(MailSendWorkflowError::Provider(MailSendProviderError::PermissionRequired)) => {
                ComposerStatus::PermissionRequired
            }
            Err(MailSendWorkflowError::Provider(error)) => ComposerStatus::ProviderFailed(error),
            Err(MailSendWorkflowError::Cache(error)) => ComposerStatus::CacheFailed(error),
            Err(MailSendWorkflowError::ReplySourceNotCached) => {
                ComposerStatus::ReplySourceUnavailable
            }
            Err(MailSendWorkflowError::OperationInProgress) => {
                ComposerStatus::CacheFailed(MailCacheError::Unavailable)
            }
            Err(MailSendWorkflowError::StateUnavailable) => ComposerStatus::CacheUnavailable,
        };
    }
}
