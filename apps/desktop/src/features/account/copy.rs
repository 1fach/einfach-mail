use einfach_mail_application::{
    AccountCacheOpenError, AccountCacheRemovalError, AccountOperationError,
    AccountPermissionRequest,
};
use einfach_mail_domain::{CalendarAction, CalendarCapabilities, MailAction, MailCapabilities};

use super::state::{AccountStatus, EwsDiscoveryState, SetupIssue};

impl SetupIssue {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::MissingClientId => {
                "Account sign-in is not configured. Set EINFACH_MAIL_ENTRA_CLIENT_ID to a Microsoft public client app ID."
            }
            Self::InvalidClientId => {
                "The Microsoft public client app ID is invalid. Check EINFACH_MAIL_ENTRA_CLIENT_ID."
            }
            Self::MissingCallbackPort => {
                "Sign-in needs a registered loopback port. Set EINFACH_MAIL_ENTRA_CALLBACK_PORT to the port registered for http://127.0.0.1."
            }
            Self::InvalidCallbackPort => {
                "The OAuth callback port must be a nonzero TCP port registered for http://127.0.0.1."
            }
            Self::ExchangeSettingsUnavailable => {
                "Exchange account settings are unavailable. Check the Linux configuration directory and try again."
            }
        }
    }
}

impl AccountStatus {
    pub(super) fn error_message(error: AccountOperationError) -> &'static str {
        match error {
            AccountOperationError::SignInCancelled => "Sign-in was cancelled.",
            AccountOperationError::AccessDenied => {
                "The server denied access. Check the account details and your organization's policies."
            }
            AccountOperationError::InvalidConnectionDetails => {
                "The Exchange endpoint or sign-in details are invalid. Check them and try again."
            }
            AccountOperationError::AutodiscoverFailed => {
                "Autodiscover could not find an EWS endpoint. Enter the HTTPS address manually."
            }
            AccountOperationError::AccessTokenRejected => {
                "The server rejected the saved sign-in. Connect your account again."
            }
            AccountOperationError::SignInExpired => {
                "The saved sign-in has expired. Connect your account again."
            }
            AccountOperationError::CredentialStoreUnavailable => {
                "Linux Secret Service is unavailable. Unlock your keyring and try again."
            }
            AccountOperationError::ProfileUnavailable => {
                "The server did not return the account details needed to identify this account."
            }
            AccountOperationError::ServiceUnavailable => {
                "The account service is unavailable. Check your connection and try again."
            }
            AccountOperationError::SignInFailed => "Sign-in could not be completed. Try again.",
            AccountOperationError::AccountAlreadyConnected => "An account is already connected.",
            AccountOperationError::AccountNotConnected => "No account is connected.",
            AccountOperationError::AccountMismatch => {
                "Microsoft returned a different account. Restore the expected account before replacing this saved sign-in."
            }
            AccountOperationError::SavedAccountIdentityUnavailable => {
                "The saved sign-in has no verified account identity. Retry restore before removing it or signing in with another account."
            }
            AccountOperationError::AccountOperationInProgress => {
                "An account operation is already running. Wait for it to finish and try again."
            }
            AccountOperationError::AccountStateUnavailable => {
                "The account workflow is unavailable. Restart Einfach Mail and try again."
            }
            _ => "The account operation failed. Try again.",
        }
    }

    pub(super) fn reauthorization_error_message(error: AccountOperationError) -> &'static str {
        match error {
            AccountOperationError::SignInCancelled => "Permission review was cancelled.",
            AccountOperationError::AccountMismatch => {
                "Microsoft returned a different account. Your saved sign-in was not changed."
            }
            _ => Self::error_message(error),
        }
    }
}

impl EwsDiscoveryState {
    pub(super) fn feedback_message(&self) -> Option<&'static str> {
        match self {
            Self::Idle | Self::Confirmation(_) => None,
            Self::Searching => Some("Searching for an EWS endpoint…"),
            Self::Cancelling => Some("Cancelling discovery…"),
            Self::Found => Some("Endpoint found. Review it before connecting."),
            Self::Failed => Some(
                "Autodiscover could not find an endpoint. Enter the HTTPS EWS address manually.",
            ),
        }
    }

    pub(super) fn button_label(&self) -> &'static str {
        if self.is_busy() {
            "Discovering…"
        } else {
            "Discover endpoint"
        }
    }
}

pub(super) fn permission_review_disclosure(request: AccountPermissionRequest) -> &'static str {
    match request {
        AccountPermissionRequest::Current => {
            "Microsoft will review the existing delegated access: User.Read, Mail.Read, Calendars.Read, and offline_access."
        }
        AccountPermissionRequest::MailReadWrite => {
            "Microsoft grants Mail.ReadWrite, which can create, read, update, and delete mail in this account. Einfach Mail requests it when Microsoft denies a read-state change, follow-up flag change, message move, or message deletion. It uses the permission to set read state, update or clear follow-up flags, move messages between folders, and delete selected messages after confirmation. This permission review does not perform the action or send mail, and it does not request Mail.Send. Continue opens Microsoft in your system browser."
        }
        AccountPermissionRequest::CalendarReadWrite => {
            "Microsoft grants Calendars.ReadWrite the ability to create, read, update, and delete calendar events. Einfach Mail requests it for supported one-off timed appointments after Microsoft denies an event change. This review does not repeat the failed action. Continue opens Microsoft in your system browser. Reopen Calendar and retry the event after review."
        }
        AccountPermissionRequest::SendMail => {
            "Microsoft grants Mail.Send so Einfach Mail can send mail as this account. It does not grant permission to read or change mail. Continue opens Microsoft in your system browser."
        }
    }
}

pub(super) fn mail_access_summary(capabilities: MailCapabilities) -> String {
    let mut actions = vec!["read messages"];
    for (action, label) in [
        (MailAction::SetReadState, "mark read or unread"),
        (
            MailAction::SetFollowUpFlagStatus,
            "set or clear follow-up flags",
        ),
        (MailAction::Send, "send"),
        (MailAction::Reply, "reply"),
        (MailAction::Move, "move or archive"),
        (MailAction::Delete, "delete"),
    ] {
        if capabilities.supports(action) {
            actions.push(label);
        }
    }
    format!("Mail actions: {}.", actions.join(", "))
}

pub(super) fn calendar_actions_summary(capabilities: CalendarCapabilities) -> String {
    let mut actions = Vec::new();
    for (action, label) in [
        (CalendarAction::ViewEvents, "view events"),
        (CalendarAction::CreateEvent, "create events"),
        (CalendarAction::UpdateEvent, "update events"),
        (CalendarAction::DeleteEvent, "delete events"),
    ] {
        if capabilities.supports(action) {
            actions.push(label);
        }
    }
    if actions.is_empty() {
        "Calendar actions: unavailable.".to_owned()
    } else {
        format!("Calendar actions: {}.", actions.join(", "))
    }
}

pub(super) fn account_cache_open_error_message(error: AccountCacheOpenError) -> &'static str {
    match error {
        AccountCacheOpenError::InUse => {
            "Another Einfach Mail operation is opening or using this account cache. Wait for it to finish, or close the other account view and try again."
        }
        AccountCacheOpenError::MissingEncryptionKey => {
            "The encrypted account cache key is missing. Remove the local cache before syncing again."
        }
        AccountCacheOpenError::Unavailable => {
            "The encrypted account cache could not be opened. Check that the cache files are accessible and your Linux keyring is unlocked, then try again."
        }
        AccountCacheOpenError::InvalidData | AccountCacheOpenError::AccountMismatch => {
            "The account cache could not be opened. Try again."
        }
    }
}

pub(super) fn account_cache_removal_error_message(error: AccountCacheRemovalError) -> &'static str {
    match error {
        AccountCacheRemovalError::InUse => {
            "Another Einfach Mail operation is opening or using this account cache. Wait for it to finish, or close the other account view and try again."
        }
        AccountCacheRemovalError::OperationInProgress => {
            "A mail or calendar operation is still active. Wait for it to finish, then try signing out again."
        }
        AccountCacheRemovalError::PendingMailSend => {
            "A previous send needs review. Open Mail, check Sent Items, and resolve it before signing out."
        }
        AccountCacheRemovalError::PendingCalendarEventCreation => {
            "A calendar event may already exist in Microsoft. Open Calendar and resolve the saved operation before signing out."
        }
        AccountCacheRemovalError::MissingEncryptionKey => {
            "The encrypted account cache key is missing. Remove the local cache before syncing again."
        }
        AccountCacheRemovalError::InvalidData | AccountCacheRemovalError::AccountMismatch => {
            "The account cache could not be verified for removal. Your account is still signed in."
        }
        AccountCacheRemovalError::Unavailable => {
            "The encrypted cache could not be read or removed. Check that the cache files are accessible and your Linux keyring is unlocked, then try again."
        }
    }
}
