use einfach_mail_application::MailSendProviderError;
use einfach_mail_domain::MailSendTarget;
use gpui_kit::component::{
    ColorTokens, Disableable,
    button::{Button, ButtonVariants},
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{AnyElement, Context, IntoElement, Role, div, prelude::*};

use super::copy::{
    cache_error_text, draft_error_text, format_system_time, pending_send_copy,
    permission_error_text, send_error_text,
};
use super::{ComposerStatus, MailComposer};

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

fn composer_status_presentation(status: ComposerStatus) -> Option<ComposerStatusPresentation> {
    Some(match status {
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
            send_error_text(MailSendProviderError::PermissionRequired),
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

impl MailComposer {
    pub(super) fn render_pending(
        &self,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(marker) = self.pending_send.marker() else {
            return div().into_any_element();
        };
        let pending = &marker.pending;
        let copy = pending_send_copy(marker.certainty);
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
            .child(copy.message);
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
                    .label(copy.button_label)
                    .disabled(self.task.is_some())
                    .on_click(cx.listener(|composer, _, _, cx| composer.resolve_pending(cx))),
            );
        #[cfg(test)]
        let pending_view = pending_view.test_support();
        pending_view.into_any_element()
    }

    pub(super) fn render_status(
        &self,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let presentation = composer_status_presentation(self.status)?;
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
}
