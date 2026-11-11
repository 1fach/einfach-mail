use std::time::SystemTime;

use einfach_mail_application::{AccountOperationError, MailCacheError, MailSendProviderError};
use einfach_mail_domain::InvalidMailSubject;
use gpui_kit::Role;

use super::recovery::PendingSendCertainty;
use super::{ComposerStatus, DraftError};

pub(super) struct ComposerStatusPresentation {
    pub(super) role: Role,
    pub(super) text: String,
    pub(super) action: Option<ComposerStatusAction>,
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

pub(super) enum ComposerStatusAction {
    ReviewPermission,
    ConfirmDuplicateRisk,
}

pub(super) struct PendingSendCopy {
    pub(super) message: String,
    pub(super) button_label: &'static str,
}

pub(super) fn composer_status_presentation(
    status: ComposerStatus,
) -> Option<ComposerStatusPresentation> {
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

pub(super) fn pending_send_copy(certainty: PendingSendCertainty) -> PendingSendCopy {
    let (message, button_label) = match certainty {
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
    PendingSendCopy {
        message,
        button_label,
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
        MailSendProviderError::UnsupportedOperation => {
            "This Exchange account is read-only and cannot send mail."
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

pub(super) fn format_system_time(value: SystemTime) -> String {
    let timestamp = time::OffsetDateTime::from(value);
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "time unavailable".to_owned())
}
