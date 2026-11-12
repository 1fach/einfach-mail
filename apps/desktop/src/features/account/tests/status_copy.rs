use super::*;

#[test]
fn calendar_summary_lists_the_provider_supported_actions() {
    assert_eq!(
        calendar_actions_summary(CalendarCapabilities::unavailable()),
        "Calendar actions: unavailable."
    );
    assert_eq!(
        calendar_actions_summary(CalendarCapabilities::view_only()),
        "Calendar actions: view events."
    );
    assert_eq!(
        calendar_actions_summary(CalendarCapabilities::from_actions([
            CalendarAction::DeleteEvent,
        ])),
        "Calendar actions: view events, delete events."
    );
    assert_eq!(
        calendar_actions_summary(CalendarCapabilities::full_access()),
        "Calendar actions: view events, create events, update events, delete events."
    );
}

#[test]
fn cancelled_permission_review_has_permission_specific_copy() {
    assert_eq!(
        AccountStatus::reauthorization_error_message(AccountOperationError::SignInCancelled),
        "Permission review was cancelled."
    );
}

#[test]
fn in_use_account_cache_message_explains_how_to_retry() {
    let message = super::super::account_cache_open_error_message(AccountCacheOpenError::InUse);

    assert!(message.contains("Another Einfach Mail operation"));
    assert!(message.contains("close the other account view"));
    assert!(message.contains("try again"));
}

#[test]
fn unavailable_account_cache_message_covers_file_and_keyring_recovery() {
    let message =
        super::super::account_cache_open_error_message(AccountCacheOpenError::Unavailable);

    assert!(message.contains("cache files"));
    assert!(message.contains("Linux keyring"));
    assert!(message.contains("try again"));
    assert!(!message.contains("removed"));
}

#[test]
fn cache_removal_validation_failure_explains_sign_in_remains() {
    for error in [
        AccountCacheRemovalError::InvalidData,
        AccountCacheRemovalError::AccountMismatch,
    ] {
        let message = super::super::account_cache_removal_error_message(error);

        assert!(message.contains("verified for removal"));
        assert!(message.contains("still signed in"));
    }
}
