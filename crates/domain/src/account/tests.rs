use super::{
    AccountCapabilities, AccountProfile, CalendarAction, CalendarCapabilities, MailAction,
    MailCapabilities, MissingAccountLabel,
};
use crate::AccountId;

fn account_id() -> AccountId {
    AccountId::try_from("account-id".to_owned()).expect("synthetic ID is non-empty")
}

#[test]
fn prefers_display_name_for_the_account_label() {
    let profile = AccountProfile::try_new(
        account_id(),
        Some("  Ada Example  ".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("profile includes display values");

    assert_eq!(profile.label(), "Ada Example");
    assert_eq!(
        profile.mailbox().map(|mailbox| mailbox.address()),
        Some("ada@example.test")
    );
    assert_eq!(
        profile.user_principal_name(),
        Some("ada@tenant.example.test")
    );
}

#[test]
fn falls_back_to_mailbox_then_user_principal_name() {
    let mailbox_profile = AccountProfile::try_new(
        account_id(),
        None,
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("mailbox identifies the account");
    let upn_profile = AccountProfile::try_new(
        account_id(),
        None,
        None,
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("user principal name identifies the account");

    assert_eq!(mailbox_profile.label(), "ada@example.test");
    assert_eq!(upn_profile.label(), "ada@tenant.example.test");
}

#[test]
fn rejects_profiles_without_a_user_facing_identity() {
    let profile_result = AccountProfile::try_new(
        account_id(),
        Some("  ".to_owned()),
        None,
        None,
        AccountCapabilities::full_access(),
    );

    assert_eq!(profile_result, Err(MissingAccountLabel));
}

#[test]
fn account_profile_debug_output_redacts_identity_values() {
    let id = AccountId::try_from("account-id-marker".to_owned())
        .expect("synthetic account ID is non-empty");
    let profile = AccountProfile::try_new(
        id,
        Some("account-display-marker".to_owned()),
        Some("mailbox-address-marker@example.test".to_owned()),
        Some("user-principal-marker@example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid");

    let debug = format!("{profile:?}");

    for marker in [
        "account-id-marker",
        "account-display-marker",
        "mailbox-address-marker",
        "user-principal-marker",
    ] {
        assert!(!debug.contains(marker), "Debug output included {marker:?}");
    }
}

#[test]
fn profile_retains_read_only_mail_capabilities() {
    let profile = AccountProfile::try_new(
        account_id(),
        None,
        Some("ada@example.test".to_owned()),
        None,
        AccountCapabilities::read_only_mail(),
    )
    .expect("mailbox identifies the account");

    assert!(profile.capabilities().mail().is_read_only());
    assert!(!profile.capabilities().can_review_mail_write_permission());
    assert_eq!(
        profile.capabilities().calendar(),
        CalendarCapabilities::unavailable()
    );
}

#[test]
fn mail_capabilities_only_include_the_requested_actions() {
    let capabilities = MailCapabilities::from_actions([MailAction::SetReadState]);

    assert!(!capabilities.is_read_only());
    assert!(capabilities.supports(MailAction::SetReadState));
    assert!(!capabilities.supports(MailAction::SetFollowUpFlagStatus));
    assert!(!capabilities.supports(MailAction::Send));
    assert!(!capabilities.supports(MailAction::Reply));
    assert!(!capabilities.supports(MailAction::Move));
    assert!(!capabilities.supports(MailAction::Delete));

    let account = AccountCapabilities::new(capabilities, CalendarCapabilities::view_only());
    assert!(!account.can_review_mail_write_permission());
}

#[test]
fn calendar_capabilities_distinguish_unavailable_and_view_only() {
    let unavailable = CalendarCapabilities::unavailable();
    assert!(!unavailable.supports(CalendarAction::ViewEvents));
    assert!(!unavailable.is_read_only());

    let view_only = CalendarCapabilities::view_only();
    assert!(view_only.supports(CalendarAction::ViewEvents));
    assert!(!view_only.supports(CalendarAction::CreateEvent));
    assert!(!view_only.supports(CalendarAction::UpdateEvent));
    assert!(!view_only.supports(CalendarAction::DeleteEvent));
    assert!(view_only.is_read_only());
}

#[test]
fn every_calendar_write_capability_also_enables_event_viewing() {
    for action in [
        CalendarAction::CreateEvent,
        CalendarAction::UpdateEvent,
        CalendarAction::DeleteEvent,
    ] {
        let capabilities = CalendarCapabilities::from_actions([action]);

        assert!(capabilities.supports(CalendarAction::ViewEvents));
        assert!(capabilities.supports(action));
        assert!(!capabilities.is_read_only());
    }
}

#[test]
fn full_access_includes_all_actions_and_permission_review() {
    let capabilities = AccountCapabilities::full_access();
    let mail = capabilities.mail();

    assert!(mail.supports(MailAction::SetReadState));
    assert!(mail.supports(MailAction::SetFollowUpFlagStatus));
    assert!(mail.supports(MailAction::Send));
    assert!(mail.supports(MailAction::Reply));
    assert!(mail.supports(MailAction::Move));
    assert!(mail.supports(MailAction::Delete));
    assert!(capabilities.can_review_mail_write_permission());

    let calendar = capabilities.calendar();
    assert!(calendar.supports(CalendarAction::ViewEvents));
    assert!(calendar.supports(CalendarAction::CreateEvent));
    assert!(calendar.supports(CalendarAction::UpdateEvent));
    assert!(calendar.supports(CalendarAction::DeleteEvent));
}
