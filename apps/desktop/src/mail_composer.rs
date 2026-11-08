use std::{future::Future, pin::Pin, sync::Arc, time::SystemTime};

use einfach_mail_application::{
    AccountOperationError, AccountPermissionRequest, AccountProvider, AccountWorkflow,
    MailCacheError, MailSendOutcome, MailSendProviderError, MailSendResolutionError,
    MailSendWorkflowError, MailSession,
};
use einfach_mail_domain::{
    AccountId, InvalidMailSubject, MailDraft, MailRecipient, MailReplyDraft, MailReplySource,
    MailSendTarget, MailSubject, PendingMailSend,
};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState, Textarea, TextareaState},
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, Entity, EventEmitter, IntoElement, KeyBinding, Rems,
    Render, Role, Task, Window, div, prelude::*, rems,
};

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

struct ComposerFields {
    recipient: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
}

enum PendingSendState {
    Available(Option<PendingSendMarker>),
    Unavailable(Option<PendingSendMarker>),
}

#[derive(Clone)]
struct PendingSendMarker {
    pending: PendingMailSend,
    certainty: PendingSendCertainty,
}

enum ComposerSubmission {
    NewMessage(MailDraft),
    Reply(MailReplyDraft),
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

struct ComposerStatusPresentation {
    role: Role,
    text: String,
    action: Option<ComposerStatusAction>,
}

impl ComposerStatusPresentation {
    fn new(role: Role, text: impl Into<String>, action: Option<ComposerStatusAction>) -> Self {
        Self {
            role,
            text: text.into(),
            action,
        }
    }
}

enum ComposerStatusAction {
    ReviewPermission,
    ConfirmDuplicateRisk,
}

#[derive(Clone, Copy)]
enum PendingSendCertainty {
    Accepted,
    Rejected(MailSendProviderError),
    Unknown,
}

#[derive(Clone, Copy)]
enum DraftError {
    InvalidRecipient,
    InvalidSubject(InvalidMailSubject),
}

impl PendingSendState {
    fn from_result(pending: Result<Option<PendingMailSend>, MailCacheError>) -> Self {
        match pending {
            Ok(pending) => Self::Available(pending.map(|pending| PendingSendMarker {
                pending,
                certainty: PendingSendCertainty::Unknown,
            })),
            Err(_) => Self::Unavailable(None),
        }
    }

    fn marker(&self) -> Option<&PendingSendMarker> {
        match self {
            Self::Available(marker) | Self::Unavailable(marker) => marker.as_ref(),
        }
    }

    fn pending(&self) -> Option<&PendingMailSend> {
        self.marker().map(|marker| &marker.pending)
    }

    fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    fn set_marker(&mut self, pending: PendingMailSend, certainty: PendingSendCertainty) {
        let marker = Some(PendingSendMarker { pending, certainty });
        *self = match self {
            Self::Available(_) => Self::Available(marker),
            Self::Unavailable(_) => Self::Unavailable(marker),
        };
    }

    fn clear_marker(&mut self) {
        *self = match self {
            Self::Available(_) => Self::Available(None),
            Self::Unavailable(_) => Self::Unavailable(None),
        };
    }

    fn apply_snapshot(&mut self, pending_result: Result<Option<PendingMailSend>, MailCacheError>) {
        match pending_result {
            Ok(pending) => {
                let marker = pending.map(|pending| {
                    let certainty = self
                        .marker()
                        .filter(|marker| marker.pending.operation_id() == pending.operation_id())
                        .map_or(PendingSendCertainty::Unknown, |marker| marker.certainty);
                    PendingSendMarker { pending, certainty }
                });
                *self = Self::Available(marker);
            }
            Err(_) => {
                let marker = self.marker().cloned().map(|mut marker| {
                    marker.certainty = PendingSendCertainty::Unknown;
                    marker
                });
                *self = Self::Unavailable(marker);
            }
        }
    }
}

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
    pub(crate) fn new(
        context: MailComposerContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fields = Self::new_fields(&context.target, window, cx);
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

    fn new_fields(
        target: &ComposerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ComposerFields {
        let recipient = cx.new(|cx| InputState::new(window, cx).placeholder("name@example.com"));
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Subject"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(match target {
                    ComposerTarget::NewMessage => "Write a plain-text message",
                    ComposerTarget::Reply { .. } => "Write a plain-text reply",
                })
                .rows(8)
        });
        ComposerFields {
            recipient,
            subject,
            body,
        }
    }

    pub(crate) fn operation_in_progress(&self) -> bool {
        self.task.is_some()
    }

    fn build_submission(&self, cx: &gpui_kit::App) -> Result<ComposerSubmission, DraftError> {
        let body = self.body.read(cx).value().to_string();
        match &self.target {
            ComposerTarget::NewMessage => self.build_new_message(cx, body),
            ComposerTarget::Reply { source, .. } => Ok(ComposerSubmission::Reply(
                MailReplyDraft::new(source.clone(), body),
            )),
        }
    }

    fn build_new_message(
        &self,
        cx: &gpui_kit::App,
        body: String,
    ) -> Result<ComposerSubmission, DraftError> {
        let recipient = self
            .recipient
            .read(cx)
            .value()
            .parse::<MailRecipient>()
            .map_err(|_| DraftError::InvalidRecipient)?;
        let subject = self
            .subject
            .read(cx)
            .value()
            .parse::<MailSubject>()
            .map_err(DraftError::InvalidSubject)?;
        Ok(ComposerSubmission::NewMessage(MailDraft::new(
            recipient, subject, body,
        )))
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

    fn submit(&mut self, cx: &mut Context<Self>) {
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
            Ok(MailSendOutcome::OutcomeUnknown(pending)) => {
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
            Err(MailSendWorkflowError::PendingResolutionRequired(pending)) => {
                self.pending_send
                    .set_marker(pending, PendingSendCertainty::Unknown);
                ComposerStatus::OutcomeUnknown
            }
            Err(MailSendWorkflowError::OperationInProgress) => {
                ComposerStatus::CacheFailed(MailCacheError::Unavailable)
            }
            Err(MailSendWorkflowError::StateUnavailable) => ComposerStatus::CacheUnavailable,
        };
    }

    fn request_permission(&mut self, cx: &mut Context<Self>) {
        let permission_requester = Arc::clone(&self.permission_requester);
        let account_id = self.account_id.clone();
        self.status = ComposerStatus::RenewingPermission;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = permission_requester
                .request_send_permission(&account_id)
                .await;
            let _ = this.update(cx, |composer, cx| {
                composer.task = None;
                composer.status = match result {
                    Ok(()) => ComposerStatus::PermissionUpdated,
                    Err(error) => ComposerStatus::PermissionRenewalFailed(error),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn resolve_pending(&mut self, cx: &mut Context<Self>) {
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

    fn confirm_duplicate_risk(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, ComposerStatus::DuplicateConfirmation) {
            return;
        }
        self.status = ComposerStatus::Editing;
        cx.notify();
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

    fn render_pending(&self, colors: ColorTokens, cx: &mut Context<Self>) -> AnyElement {
        let Some(marker) = self.pending_send.marker() else {
            return div().into_any_element();
        };
        let pending = &marker.pending;
        let (copy, button_label) = match marker.certainty {
            PendingSendCertainty::Accepted => (
                "Microsoft accepted this message for processing. The local send marker could not be cleared."
                    .to_owned(),
                "Clear accepted send marker",
            ),
            PendingSendCertainty::Rejected(error) => (
                format!(
                    "{} The local send marker could not be cleared.",
                    send_error_text(error)
                ),
                "Clear rejected send marker",
            ),
            PendingSendCertainty::Unknown => (
                "Microsoft may have accepted this message. Check Sent Items before deciding whether to send another copy."
                    .to_owned(),
                "I checked Sent Items",
            ),
        };
        let mut pending_view = div()
            .id("mail-pending-send")
            .debug_selector(|| "mail-pending-send".to_owned())
            .role(Role::Alert)
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(colors.muted)
            .text_color(colors.foreground)
            .child(copy);
        pending_view = match pending.target() {
            MailSendTarget::NewMessage { recipient, subject } => pending_view
                .child(format!("To: {}", recipient.as_str()))
                .child(format!("Subject: {}", subject.as_str())),
            MailSendTarget::Reply { subject, .. } => {
                let pending_view = pending_view.child("Reply to the source message");
                if let Some(subject) = subject {
                    pending_view.child(format!("Original subject: {subject}"))
                } else {
                    pending_view.child("Original message has no subject")
                }
            }
        };
        let pending_view = pending_view
            .child(format!(
                "Started: {}",
                format_system_time(pending.created_at())
            ))
            .child(
                Button::new("resolve-pending-mail-send")
                    .debug_selector(|| "resolve-pending-mail-send".to_owned())
                    .ghost()
                    .label(button_label)
                    .disabled(self.task.is_some())
                    .on_click(cx.listener(|composer, _, _, cx| composer.resolve_pending(cx))),
            );
        #[cfg(test)]
        let pending_view = pending_view.test_support();
        pending_view.into_any_element()
    }

    fn render_status(&self, colors: ColorTokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let presentation = self.status_presentation()?;
        let mut children = vec![presentation.text.into_any_element()];
        match presentation.action {
            Some(ComposerStatusAction::ReviewPermission) => {
                children.push(" The request is limited to sending as the signed-in account. Microsoft may show a consent screen in your system browser; approval grants permission but does not send this message. Return here and press Send to submit it.".into_any_element());
                children.push(self.permission_review_button(cx));
            }
            Some(ComposerStatusAction::ConfirmDuplicateRisk) => {
                children.push(self.duplicate_confirmation_button(cx));
            }
            None => {}
        }
        let feedback = div()
            .id("mail-composer-status")
            .debug_selector(|| "mail-composer-status".to_owned())
            .role(presentation.role)
            .text_sm()
            .text_color(if presentation.role == Role::Alert {
                colors.destructive
            } else {
                colors.muted_foreground
            })
            .children(children);
        #[cfg(test)]
        let feedback = feedback.test_support();
        Some(feedback.into_any_element())
    }

    fn status_presentation(&self) -> Option<ComposerStatusPresentation> {
        Some(match self.status {
            ComposerStatus::Editing => return None,
            ComposerStatus::InvalidDraft(error) => {
                ComposerStatusPresentation::new(Role::Alert, draft_error_text(error), None)
            }
            ComposerStatus::Sending => {
                ComposerStatusPresentation::new(Role::Status, "Sending message…", None)
            }
            ComposerStatus::Accepted => ComposerStatusPresentation::new(
                Role::Status,
                "Microsoft accepted the message for processing. This does not confirm delivery.",
                None,
            ),
            ComposerStatus::AcceptedNeedsResolution => ComposerStatusPresentation::new(
                Role::Status,
                "Microsoft accepted the message, but the local send marker remains. Clear it before sending another message.",
                None,
            ),
            ComposerStatus::RejectedNeedsResolution(error) => ComposerStatusPresentation::new(
                Role::Alert,
                format!(
                    "Microsoft rejected this message, but the local send marker could not be cleared. {}",
                    cache_error_text(error)
                ),
                None,
            ),
            ComposerStatus::OutcomeUnknown => ComposerStatusPresentation::new(
                Role::Alert,
                "The send outcome is unknown. The app will not resend this message automatically. Check Sent Items.",
                None,
            ),
            ComposerStatus::PermissionRequired => ComposerStatusPresentation::new(
                Role::Alert,
                "Microsoft denied send permission. Review the delegated Mail.Send request to continue.",
                Some(ComposerStatusAction::ReviewPermission),
            ),
            ComposerStatus::RenewingPermission => ComposerStatusPresentation::new(
                Role::Status,
                "Waiting for Microsoft permission review…",
                None,
            ),
            ComposerStatus::PermissionUpdated => ComposerStatusPresentation::new(
                Role::Status,
                "Permission updated. No message was sent. Press Send to submit this message.",
                None,
            ),
            ComposerStatus::PermissionRenewalFailed(error) => {
                ComposerStatusPresentation::new(Role::Alert, permission_error_text(error), None)
            }
            ComposerStatus::ProviderFailed(error) => {
                ComposerStatusPresentation::new(Role::Alert, send_error_text(error), None)
            }
            ComposerStatus::ReplySourceUnavailable => ComposerStatusPresentation::new(
                Role::Alert,
                "The original message is no longer in the local cache. Reopen it before replying.",
                None,
            ),
            ComposerStatus::CacheFailed(error) => {
                ComposerStatusPresentation::new(Role::Alert, cache_error_text(error), None)
            }
            ComposerStatus::CacheUnavailable => ComposerStatusPresentation::new(
                Role::Alert,
                "The local encrypted cache could not confirm send status. Resolve cache access before another send.",
                None,
            ),
            ComposerStatus::DuplicateConfirmation => ComposerStatusPresentation::new(
                Role::Alert,
                "The earlier message may still arrive later. Sending again could create a duplicate.",
                Some(ComposerStatusAction::ConfirmDuplicateRisk),
            ),
        })
    }

    fn permission_review_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("review-send-permission")
            .ghost()
            .debug_selector(|| "review-send-permission".to_owned())
            .label("Review Microsoft send permission…")
            .disabled(self.task.is_some() || self.pending_send.pending().is_some())
            .on_click(cx.listener(|composer, _, _, cx| composer.request_permission(cx)))
            .into_any_element()
    }

    fn duplicate_confirmation_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("confirm-duplicate-risk")
            .debug_selector(|| "confirm-duplicate-risk".to_owned())
            .label("Allow another send")
            .disabled(self.task.is_some())
            .on_click(cx.listener(|composer, _, _, cx| composer.confirm_duplicate_risk(cx)))
            .into_any_element()
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
        let mut children = vec![self.render_pending(colors, cx)];
        children.extend(self.render_target_fields(fields_disabled));
        children.push(self.render_body_field(fields_disabled));
        children.extend(self.render_status(colors, cx));
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

fn draft_error_text(error: DraftError) -> String {
    match error {
        DraftError::InvalidRecipient => "Enter one valid email address.".to_owned(),
        DraftError::InvalidSubject(InvalidMailSubject::Empty) => "Enter a subject.".to_owned(),
        DraftError::InvalidSubject(InvalidMailSubject::Multiline) => {
            "Keep the subject on one line.".to_owned()
        }
    }
}

fn permission_error_text(error: AccountOperationError) -> &'static str {
    match error {
        AccountOperationError::SignInCancelled => "Microsoft permission review was cancelled.",
        AccountOperationError::AccessDenied => "Microsoft or your organization denied Mail.Send.",
        AccountOperationError::AccountMismatch => {
            "Microsoft returned a different account. The active account was not changed."
        }
        AccountOperationError::CredentialStoreUnavailable => {
            "Linux Secret Service could not save the renewed sign-in."
        }
        _ => "Microsoft permission review failed. Your message remains in this composer.",
    }
}

fn send_error_text(error: MailSendProviderError) -> &'static str {
    match error {
        MailSendProviderError::SignInRequired => {
            "Your Microsoft sign-in needs attention. Return to Accounts and reconnect."
        }
        MailSendProviderError::AccountMismatch => {
            "The active Microsoft account changed. Return to Accounts before sending."
        }
        MailSendProviderError::PermissionRequired => {
            "Microsoft denied send permission. Review the delegated Mail.Send request to continue."
        }
        MailSendProviderError::Rejected => {
            "Microsoft explicitly rejected this message. Check the recipient and subject before trying again."
        }
        MailSendProviderError::RateLimited { .. } => {
            "Microsoft rejected this send because the account is being throttled. Wait before trying again."
        }
        MailSendProviderError::ServiceUnavailable => {
            "The mail service is unavailable. Your message remains in this composer."
        }
        MailSendProviderError::OutcomeUnknown => {
            "The send outcome is unknown. Check Sent Items before deciding whether to send another copy."
        }
    }
}

fn status_after_rejection(error: MailSendProviderError) -> ComposerStatus {
    if matches!(error, MailSendProviderError::PermissionRequired) {
        ComposerStatus::PermissionRequired
    } else {
        ComposerStatus::ProviderFailed(error)
    }
}

fn cache_error_text(error: MailCacheError) -> &'static str {
    match error {
        MailCacheError::AccountMismatch => "The pending send belongs to another account.",
        MailCacheError::SendAlreadyPending => {
            "A previous send needs review before another message can be submitted."
        }
        MailCacheError::PendingSendMismatch | MailCacheError::PendingSendNotFound => {
            "The pending send changed. Reload mail before continuing."
        }
        MailCacheError::MissingEncryptionKey => {
            "The encrypted mail cache key is unavailable. Restore cache access before sending."
        }
        MailCacheError::InUse => {
            "The account cache is being used by another operation. Wait for it to finish before sending."
        }
        MailCacheError::Unavailable => {
            "The encrypted mail cache is unavailable. Restore cache access before sending."
        }
        MailCacheError::SyncStateChanged => {
            "The folder changed during this send. Reload mail before continuing."
        }
        MailCacheError::InvalidData | MailCacheError::MessageNotCached => {
            "The local send status is invalid. Reload mail before sending."
        }
    }
}

fn format_system_time(value: SystemTime) -> String {
    let timestamp = time::OffsetDateTime::from(value);
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "time unavailable".to_owned())
}
