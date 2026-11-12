mod copy;
mod draft;
mod form;
mod permission;
mod presentation;
mod recovery;
mod resolution;
mod submission;

use std::{future::Future, pin::Pin, sync::Arc};

use draft::{ComposerFields, DraftError};
use einfach_mail_application::{
    AccountOperationError, AccountPermissionRequest, AccountProvider, AccountWorkflow,
    MailCacheError, MailSendProviderError, MailSession,
};
use einfach_mail_domain::{AccountId, MailReplySource, PendingMailSend};
use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::{App, Context, Entity, EventEmitter, KeyBinding, Task, Window};
use recovery::PendingSendState;

const MAIL_COMPOSER_WRITE_PENDING_CONTEXT: &str = "MailComposerWritePending";

pub(crate) fn bind_mail_composer_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        gpui_kit::NoAction,
        Some(MAIL_COMPOSER_WRITE_PENDING_CONTEXT),
    )]);
}

pub(crate) enum MailComposerEvent {
    PendingSendChanged(Option<PendingMailSend>),
}

pub(crate) trait MailPermissionRequester: Send + Sync {
    fn request_send_permission<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> Pin<Box<dyn Future<Output = Result<(), AccountOperationError>> + Send + 'a>>;
}

impl<P: AccountProvider + 'static> MailPermissionRequester for AccountWorkflow<P> {
    fn request_send_permission<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> Pin<Box<dyn Future<Output = Result<(), AccountOperationError>> + Send + 'a>> {
        Box::pin(async move {
            self.reauthorize_account(account_id, AccountPermissionRequest::SendMail)
                .await
                .map(|_| ())
        })
    }
}

pub(crate) struct MailComposer {
    workflow: Arc<dyn MailSession>,
    permission_requester: Arc<dyn MailPermissionRequester>,
    account_id: AccountId,
    target: ComposerTarget,
    recipient: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    pending_send: PendingSendState,
    status: ComposerStatus,
    task: Option<Task<()>>,
}

#[derive(Clone)]
pub(super) enum ComposerTarget {
    NewMessage,
    Reply {
        source: MailReplySource,
        subject: Option<String>,
    },
}

pub(super) struct MailComposerContext {
    pub(super) workflow: Arc<dyn MailSession>,
    pub(super) permission_requester: Arc<dyn MailPermissionRequester>,
    pub(super) account_id: AccountId,
    pub(super) pending: Result<Option<PendingMailSend>, MailCacheError>,
    pub(super) target: ComposerTarget,
}

#[derive(Clone, Copy)]
enum ComposerStatus {
    Editing,
    InvalidDraft(DraftError),
    Sending,
    Accepted,
    AcceptedNeedsResolution,
    RejectedNeedsResolution(MailCacheError),
    OutcomeUnknown,
    PermissionRequired,
    RenewingPermission,
    PermissionUpdated,
    PermissionRenewalFailed(AccountOperationError),
    ProviderFailed(MailSendProviderError),
    ReplySourceUnavailable,
    CacheFailed(MailCacheError),
    CacheUnavailable,
    DuplicateConfirmation,
}

impl MailComposer {
    pub(crate) fn new(
        context: MailComposerContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fields = ComposerFields::new(&context.target, window, cx);
        Self::from_context(context, fields)
    }

    fn from_context(context: MailComposerContext, fields: ComposerFields) -> Self {
        let pending_send = PendingSendState::from_result(context.pending);
        let status = if pending_send.is_available() {
            ComposerStatus::Editing
        } else {
            ComposerStatus::CacheUnavailable
        };
        Self {
            workflow: context.workflow,
            permission_requester: context.permission_requester,
            account_id: context.account_id,
            target: context.target,
            recipient: fields.recipient,
            subject: fields.subject,
            body: fields.body,
            pending_send,
            status,
            task: None,
        }
    }

    pub(crate) fn operation_in_progress(&self) -> bool {
        self.task.is_some()
    }
}

impl EventEmitter<MailComposerEvent> for MailComposer {}

fn status_after_rejection(error: MailSendProviderError) -> ComposerStatus {
    if matches!(error, MailSendProviderError::PermissionRequired) {
        ComposerStatus::PermissionRequired
    } else {
        ComposerStatus::ProviderFailed(error)
    }
}
