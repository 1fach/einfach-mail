use std::time::SystemTime;

use einfach_mail_application::{AccountOperationError, MailCacheError, MailSendProviderError};
use einfach_mail_domain::InvalidMailSubject;

use super::draft::DraftError;
use super::recovery::PendingSendCertainty;

pub(super) struct PendingSendCopy {
    pub(super) message: String,
    pub(super) button_label: &'static str,
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

pub(super) fn draft_error_text(error: DraftError) -> String {
    match error {
        DraftError::InvalidRecipient => "Enter one valid email address.".to_owned(),
        DraftError::InvalidSubject(InvalidMailSubject::Empty) => "Enter a subject.".to_owned(),
        DraftError::InvalidSubject(InvalidMailSubject::Multiline) => {
            "Keep the subject on one line.".to_owned()
        }
    }
}

pub(super) fn permission_error_text(error: AccountOperationError) -> &'static str {
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

pub(super) fn send_error_text(error: MailSendProviderError) -> &'static str {
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

pub(super) fn cache_error_text(error: MailCacheError) -> &'static str {
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
