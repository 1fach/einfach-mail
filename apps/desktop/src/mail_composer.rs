use std::{future::Future, pin::Pin, sync::Arc, time::SystemTime};

use einfach_mail_application::{
    AccountOperationError, AccountPermissionRequest, AccountProvider, AccountWorkflow,
    MailCacheError, MailSendOutcome, MailSendProviderError, MailSendResolutionError,
    MailSendWorkflowError, MailSession,
};
use einfach_mail_domain::{
    AccountId, InvalidMailSubject, MailDraft, MailRecipient, MailSubject, PendingMailSend,
};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState, Textarea, TextareaState},
    scroll::ScrollableElement,
};
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, Entity, EventEmitter, IntoElement, KeyBinding, Render,
    Role, Task, Window, div, prelude::*, px,
};

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
    permission_requester: Option<Arc<dyn MailPermissionRequester>>,
    account_id: AccountId,
    recipient: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    pending: Option<PendingMailSend>,
    pending_certainty: Option<PendingSendCertainty>,
    pending_state_available: bool,
    duplicate_confirmation_required: bool,
    status: ComposerStatus,
    task: Option<Task<()>>,
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
    CacheFailed(MailCacheError),
    CacheUnavailable,
    DuplicateConfirmation,
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

impl MailComposer {
    pub(crate) fn new(
        workflow: Arc<dyn MailSession>,
        permission_requester: Option<Arc<dyn MailPermissionRequester>>,
        account_id: AccountId,
        pending: Result<Option<PendingMailSend>, MailCacheError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let recipient = cx.new(|cx| InputState::new(window, cx).placeholder("name@example.com"));
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Subject"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write a plain-text message")
                .rows(8)
        });
        let (pending, pending_state_available) = match pending {
            Ok(pending) => (pending, true),
            Err(_) => (None, false),
        };
        Self {
            workflow,
            permission_requester,
            account_id,
            recipient,
            subject,
            body,
            pending_certainty: pending.as_ref().map(|_| PendingSendCertainty::Unknown),
            pending,
            pending_state_available,
            duplicate_confirmation_required: false,
            status: if pending_state_available {
                ComposerStatus::Editing
            } else {
                ComposerStatus::CacheUnavailable
            },
            task: None,
        }
    }

    pub(crate) fn operation_in_progress(&self) -> bool {
        self.task.is_some()
    }

    fn build_draft(&self, cx: &gpui_kit::App) -> Result<MailDraft, DraftError> {
        let recipient_value = self.recipient.read(cx).value();
        let recipient = recipient_value
            .parse::<MailRecipient>()
            .map_err(|_| DraftError::InvalidRecipient)?;
        let subject = self
            .subject
            .read(cx)
            .value()
            .parse::<MailSubject>()
            .map_err(DraftError::InvalidSubject)?;
        let body = self.body.read(cx).value().to_string();
        Ok(MailDraft::new(recipient, subject, body))
    }

    fn can_send(&self) -> bool {
        self.pending.is_none()
            && self.pending_state_available
            && !self.duplicate_confirmation_required
            && !matches!(
                self.status,
                ComposerStatus::Sending | ComposerStatus::RenewingPermission
            )
            && !matches!(self.status, ComposerStatus::Accepted)
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_send() {
            return;
        }
        let draft = match self.build_draft(cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.status = ComposerStatus::InvalidDraft(error);
                cx.notify();
                return;
            }
        };
        let workflow = Arc::clone(&self.workflow);
        let account_id = self.account_id.clone();
        self.status = ComposerStatus::Sending;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = workflow
                .send_message(&account_id, draft, SystemTime::now())
                .await;
            let pending_result = workflow.load_pending_send(&account_id).await;
            let _ = this.update(cx, |composer, cx| {
                composer.apply_send_result(result, pending_result, cx);
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
        if self.pending_state_available || workflow_result_survives_missing_snapshot {
            self.apply_send_workflow_result(result);
        }
        cx.emit(MailComposerEvent::PendingSendChanged(self.pending.clone()));
        cx.notify();
    }

    fn apply_pending_snapshot(
        &mut self,
        pending_result: Result<Option<PendingMailSend>, MailCacheError>,
    ) {
        match pending_result {
            Ok(pending) => {
                let previous_operation_id =
                    self.pending.as_ref().map(PendingMailSend::operation_id);
                let certainty_matches = pending
                    .as_ref()
                    .is_some_and(|pending| Some(pending.operation_id()) == previous_operation_id);
                if !certainty_matches {
                    self.pending_certainty =
                        pending.as_ref().map(|_| PendingSendCertainty::Unknown);
                }
                self.pending_state_available = true;
                self.pending = pending;
            }
            Err(_) => {
                self.pending_state_available = false;
                self.pending_certainty =
                    self.pending.as_ref().map(|_| PendingSendCertainty::Unknown);
                self.status = ComposerStatus::CacheUnavailable;
            }
        }
    }

    fn apply_send_workflow_result(
        &mut self,
        result: Result<MailSendOutcome, MailSendWorkflowError>,
    ) {
        self.status = match result {
            Ok(MailSendOutcome::Accepted) => {
                self.pending = None;
                self.pending_certainty = None;
                ComposerStatus::Accepted
            }
            Ok(MailSendOutcome::AcceptedNeedsResolution(pending)) => {
                self.pending = Some(pending);
                self.pending_certainty = Some(PendingSendCertainty::Accepted);
                ComposerStatus::AcceptedNeedsResolution
            }
            Ok(MailSendOutcome::OutcomeUnknown(pending)) => {
                self.pending = Some(pending);
                self.pending_certainty = Some(PendingSendCertainty::Unknown);
                ComposerStatus::OutcomeUnknown
            }
            Err(MailSendWorkflowError::RejectedNeedsResolution {
                pending,
                provider,
                cache,
            }) => {
                self.pending = Some(pending);
                self.pending_certainty = Some(PendingSendCertainty::Rejected(provider));
                ComposerStatus::RejectedNeedsResolution(cache)
            }
            Err(MailSendWorkflowError::Provider(MailSendProviderError::PermissionRequired)) => {
                ComposerStatus::PermissionRequired
            }
            Err(MailSendWorkflowError::Provider(error)) => ComposerStatus::ProviderFailed(error),
            Err(MailSendWorkflowError::Cache(error)) => ComposerStatus::CacheFailed(error),
            Err(MailSendWorkflowError::PendingResolutionRequired(pending)) => {
                self.pending = Some(pending);
                self.pending_certainty = Some(PendingSendCertainty::Unknown);
                ComposerStatus::OutcomeUnknown
            }
            Err(MailSendWorkflowError::OperationInProgress) => {
                ComposerStatus::CacheFailed(MailCacheError::Unavailable)
            }
            Err(MailSendWorkflowError::StateUnavailable) => ComposerStatus::CacheUnavailable,
        };
    }

    fn request_permission(&mut self, cx: &mut Context<Self>) {
        let Some(permission_requester) = self.permission_requester.clone() else {
            self.status = ComposerStatus::PermissionRenewalFailed(
                AccountOperationError::AccountStateUnavailable,
            );
            cx.notify();
            return;
        };
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
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if self.task.is_some() {
            return;
        }
        let workflow = Arc::clone(&self.workflow);
        let account_id = self.account_id.clone();
        let operation_id = pending.operation_id();
        let certainty = self
            .pending_certainty
            .unwrap_or(PendingSendCertainty::Unknown);
        self.status = ComposerStatus::Sending;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = workflow
                .resolve_pending_send(&account_id, operation_id)
                .await;
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
                self.pending = Some(pending);
                self.pending_certainty = Some(PendingSendCertainty::Unknown);
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
        self.pending = None;
        self.pending_certainty = None;
        self.pending_state_available = true;
        self.duplicate_confirmation_required = matches!(certainty, PendingSendCertainty::Unknown);
        self.status = match certainty {
            PendingSendCertainty::Accepted => ComposerStatus::Accepted,
            PendingSendCertainty::Rejected(error) => status_after_rejection(error),
            PendingSendCertainty::Unknown => ComposerStatus::DuplicateConfirmation,
        };
        cx.emit(MailComposerEvent::PendingSendChanged(None));
    }

    fn confirm_duplicate_risk(&mut self, cx: &mut Context<Self>) {
        if !self.duplicate_confirmation_required {
            return;
        }
        self.duplicate_confirmation_required = false;
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
        let Some(pending) = &self.pending else {
            return div().into_any_element();
        };
        let certainty = self
            .pending_certainty
            .unwrap_or(PendingSendCertainty::Unknown);
        let (copy, button_label) = match certainty {
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
        div()
            .id("mail-pending-send")
            .debug_selector(|| "mail-pending-send".to_owned())
            .role(Role::Alert)
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(colors.muted)
            .text_color(colors.foreground)
            .child(copy)
            .child(format!("To: {}", pending.recipient().as_str()))
            .child(format!("Subject: {}", pending.subject()))
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
            )
            .into_any_element()
    }

    fn render_status(&self, colors: ColorTokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (role, text) = match self.status {
            ComposerStatus::Editing => return None,
            ComposerStatus::InvalidDraft(error) => (Role::Alert, draft_error_text(error)),
            ComposerStatus::Sending => (Role::Status, "Sending message…".to_owned()),
            ComposerStatus::Accepted => (
                Role::Status,
                "Microsoft accepted the message for processing. This does not confirm delivery."
                    .to_owned(),
            ),
            ComposerStatus::AcceptedNeedsResolution => (
                Role::Status,
                "Microsoft accepted the message, but the local send marker remains. Clear it before sending another message."
                    .to_owned(),
            ),
            ComposerStatus::RejectedNeedsResolution(error) => (
                Role::Alert,
                format!(
                    "Microsoft rejected this message, but the local send marker could not be cleared. {}",
                    cache_error_text(error)
                ),
            ),
            ComposerStatus::OutcomeUnknown => (
                Role::Alert,
                "The send outcome is unknown. The app will not resend this message automatically. Check Sent Items."
                    .to_owned(),
            ),
            ComposerStatus::PermissionRequired => (
                Role::Alert,
                "Microsoft denied send permission. Review the delegated Mail.Send request to continue."
                    .to_owned(),
            ),
            ComposerStatus::RenewingPermission => (
                Role::Status,
                "Waiting for Microsoft permission review…".to_owned(),
            ),
            ComposerStatus::PermissionUpdated => (
                Role::Status,
                "Permission updated. No message was sent. Press Send to submit this message."
                    .to_owned(),
            ),
            ComposerStatus::PermissionRenewalFailed(error) => {
                (Role::Alert, permission_error_text(error).to_owned())
            }
            ComposerStatus::ProviderFailed(error) => (Role::Alert, send_error_text(error).to_owned()),
            ComposerStatus::CacheFailed(error) => (Role::Alert, cache_error_text(error).to_owned()),
            ComposerStatus::CacheUnavailable => (
                Role::Alert,
                "The local encrypted cache could not confirm send status. Resolve cache access before another send."
                    .to_owned(),
            ),
            ComposerStatus::DuplicateConfirmation => (
                Role::Alert,
                "The earlier message may still arrive later. Sending again could create a duplicate."
                    .to_owned(),
            ),
        };
        let feedback = div()
            .id("mail-composer-status")
            .debug_selector(|| "mail-composer-status".to_owned())
            .role(role)
            .text_sm()
            .text_color(if role == Role::Alert {
                colors.destructive
            } else {
                colors.muted_foreground
            })
            .child(text);
        let feedback = match self.status {
            ComposerStatus::PermissionRequired => feedback
                .child(" The request is limited to sending as the signed-in account. Microsoft may show a consent screen in your system browser; approval grants permission but does not send this message. Return here and press Send to submit it.")
                .child(
                    Button::new("review-send-permission")
                        .ghost()
                        .debug_selector(|| "review-send-permission".to_owned())
                        .label("Review Microsoft send permission…")
                        .disabled(self.task.is_some() || self.pending.is_some())
                        .on_click(cx.listener(|composer, _, _, cx| {
                            composer.request_permission(cx);
                        })),
                ),
            ComposerStatus::DuplicateConfirmation => feedback.child(
                Button::new("confirm-duplicate-risk")
                    .label("Allow another send")
                    .disabled(self.task.is_some())
                    .on_click(cx.listener(|composer, _, _, cx| {
                        composer.confirm_duplicate_risk(cx);
                    })),
            ),
            _ => feedback,
        };
        Some(feedback.into_any_element())
    }
}

impl EventEmitter<MailComposerEvent> for MailComposer {}

impl Render for MailComposer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let operation_in_progress = self.operation_in_progress();
        let fields_disabled = self.pending.is_some()
            || !self.pending_state_available
            || self.task.is_some()
            || self.duplicate_confirmation_required
            || matches!(self.status, ComposerStatus::Accepted);
        let send_disabled = !self.can_send();
        let send_label = if matches!(self.status, ComposerStatus::Sending) {
            "Sending…"
        } else {
            "Send"
        };
        let mut content = div()
            .id("mail-composer")
            .debug_selector(|| "mail-composer".to_owned())
            .size_full()
            .when(operation_in_progress, |element| {
                element.key_context(MAIL_COMPOSER_WRITE_PENDING_CONTEXT)
            })
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .overflow_y_scrollbar()
            .child(self.render_pending(colors, cx))
            .child(Self::render_field(
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
            ))
            .child(Self::render_field(
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
            ))
            .child(Self::render_field(
                "mail-body-field",
                "Message",
                div()
                    .id("mail-body-input")
                    .debug_selector(|| "mail-body-input".to_owned())
                    .child(
                        Textarea::new(&self.body)
                            .aria_label("Plain-text message")
                            .disabled(fields_disabled)
                            .h(px(220.))
                            .w_full(),
                    ),
            ));
        if let Some(status) = self.render_status(colors, cx) {
            content = content.child(status);
        }
        content
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child("Messages use plain text and one recipient. The app saves no message body in its local send status."),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("mail-send")
                            .primary()
                            .debug_selector(|| "mail-send-button".to_owned())
                            .label(send_label)
                            .loading(matches!(self.status, ComposerStatus::Sending))
                            .disabled(send_disabled)
                            .on_click(cx.listener(|composer, _, _, cx| composer.submit(cx))),
                    ),
            )
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
        MailCacheError::Unavailable => {
            "The encrypted mail cache is unavailable. Restore cache access before sending."
        }
        MailCacheError::SyncStateChanged | MailCacheError::OperationInProgress => {
            "Another account operation is active. Wait for it to finish before sending."
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
