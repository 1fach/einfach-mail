mod copy;
mod draft;
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
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState, Textarea, TextareaState},
    scroll::ScrollableElement,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, IntoElement, KeyBinding, Rems, Render, Task,
    Window, div, prelude::*, rems,
};
use recovery::PendingSendState;

const MAIL_COMPOSER_WRITE_PENDING_CONTEXT: &str = "MailComposerWritePending";
const MAIL_COMPOSER_BODY_HEIGHT: Rems = rems(13.75);

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

    fn can_send(&self) -> bool {
        self.pending_send.pending().is_none()
            && self.pending_send.is_available()
            && !matches!(
                self.status,
                ComposerStatus::Sending
                    | ComposerStatus::RenewingPermission
                    | ComposerStatus::Accepted
                    | ComposerStatus::DuplicateConfirmation
            )
    }

    fn render_field(
        id: &'static str,
        label: &'static str,
        input: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().font_bold().child(label))
            .child(input)
    }

    fn fields_disabled(&self) -> bool {
        self.pending_send.pending().is_some()
            || !self.pending_send.is_available()
            || self.task.is_some()
            || matches!(
                self.status,
                ComposerStatus::Accepted | ComposerStatus::DuplicateConfirmation
            )
    }

    fn render_composer_content(
        &mut self,
        colors: ColorTokens,
        fields_disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("mail-composer")
            .debug_selector(|| "mail-composer".to_owned())
            .size_full()
            .when(self.operation_in_progress(), |element| {
                element.key_context(MAIL_COMPOSER_WRITE_PENDING_CONTEXT)
            })
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .overflow_y_scrollbar()
            .children(self.render_composer_children(colors, fields_disabled, cx))
    }

    fn render_composer_children(
        &mut self,
        colors: ColorTokens,
        fields_disabled: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut children = vec![self.render_pending(&colors, cx)];
        children.extend(self.render_target_fields(fields_disabled));
        children.push(self.render_body_field(fields_disabled));
        children.extend(self.render_status(&colors, cx));
        children.push(self.render_disclaimer(colors));
        children.push(self.render_send_action(cx));
        children
    }

    fn render_target_fields(&self, fields_disabled: bool) -> Vec<AnyElement> {
        match &self.target {
            ComposerTarget::NewMessage => vec![
                self.render_recipient_field(fields_disabled),
                self.render_subject_field(fields_disabled),
            ],
            ComposerTarget::Reply { subject, .. } => {
                vec![Self::render_reply_context(subject)]
            }
        }
    }

    fn render_recipient_field(&self, fields_disabled: bool) -> AnyElement {
        Self::render_field(
            "mail-recipient-field",
            "To",
            div()
                .id("mail-recipient-input")
                .debug_selector(|| "mail-recipient-input".to_owned())
                .child(
                    Input::new(&self.recipient)
                        .aria_label("Recipient")
                        .disabled(fields_disabled)
                        .w_full(),
                ),
        )
        .into_any_element()
    }

    fn render_subject_field(&self, fields_disabled: bool) -> AnyElement {
        Self::render_field(
            "mail-subject-field",
            "Subject",
            div()
                .id("mail-subject-input")
                .debug_selector(|| "mail-subject-input".to_owned())
                .child(
                    Input::new(&self.subject)
                        .aria_label("Subject")
                        .disabled(fields_disabled)
                        .w_full(),
                ),
        )
        .into_any_element()
    }

    fn render_reply_context(subject: &Option<String>) -> AnyElement {
        div()
            .id("mail-reply-context")
            .debug_selector(|| "mail-reply-context".to_owned())
            .text_sm()
            .child("Microsoft will use the original message's reply recipients, including Reply-To when present.")
            .children(subject.as_ref().map(|subject| {
                div().text_sm().child(format!("Original subject: {subject}"))
            }))
            .into_any_element()
    }

    fn render_body_field(&self, fields_disabled: bool) -> AnyElement {
        let (label, aria_label) = self.body_labels();
        Self::render_field(
            "mail-body-field",
            label,
            div()
                .id("mail-body-input")
                .debug_selector(|| "mail-body-input".to_owned())
                .child(
                    Textarea::new(&self.body)
                        .aria_label(aria_label)
                        .disabled(fields_disabled)
                        .h(MAIL_COMPOSER_BODY_HEIGHT)
                        .w_full(),
                ),
        )
        .into_any_element()
    }

    fn body_labels(&self) -> (&'static str, &'static str) {
        match &self.target {
            ComposerTarget::NewMessage => ("Message", "Plain-text message"),
            ComposerTarget::Reply { .. } => ("Reply", "Plain-text reply"),
        }
    }

    fn render_disclaimer(&self, colors: ColorTokens) -> AnyElement {
        let text = match &self.target {
            ComposerTarget::NewMessage => {
                "Messages use plain text and one recipient. The app saves no message body in its local send status."
            }
            ComposerTarget::Reply { .. } => {
                "Replies use plain text. The app keeps reply content in memory and saves no body in its local send status."
            }
        };
        div()
            .text_xs()
            .text_color(colors.muted_foreground)
            .child(text)
            .into_any_element()
    }

    fn render_send_action(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                Button::new("mail-send")
                    .primary()
                    .debug_selector(|| "mail-send-button".to_owned())
                    .label(if matches!(self.status, ComposerStatus::Sending) {
                        "Sending…"
                    } else {
                        "Send"
                    })
                    .loading(matches!(self.status, ComposerStatus::Sending))
                    .disabled(!self.can_send())
                    .on_click(cx.listener(|composer, _, _, cx| composer.submit(cx))),
            )
            .into_any_element()
    }
}

impl EventEmitter<MailComposerEvent> for MailComposer {}

impl Render for MailComposer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let fields_disabled = self.fields_disabled();
        self.render_composer_content(colors, fields_disabled, cx)
    }
}

fn status_after_rejection(error: MailSendProviderError) -> ComposerStatus {
    if matches!(error, MailSendProviderError::PermissionRequired) {
        ComposerStatus::PermissionRequired
    } else {
        ComposerStatus::ProviderFailed(error)
    }
}
