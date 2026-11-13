use super::{PreviewMailProvider, PreviewScenario, folder_reference, preview_capabilities};
use einfach_mail_application::{
    MailOperationError, MailProvider, MessageDeltaRequest, MessageDeltaResponse,
};
use einfach_mail_domain::{AccountId, CalendarCapabilities, MailCapabilities};

#[test]
fn preview_scenario_parser_accepts_default_recovery_and_full_access_scenarios() {
    assert_eq!(
        PreviewScenario::parse(None).expect("default preview scenario is valid"),
        PreviewScenario::Standard
    );
    assert_eq!(
        PreviewScenario::parse(Some("mail-sync-recovery"))
            .expect("mail recovery preview scenario is valid"),
        PreviewScenario::MailSyncRecovery
    );
    assert_eq!(
        PreviewScenario::parse(Some("full-access")).expect("full access preview scenario is valid"),
        PreviewScenario::FullAccess
    );
    assert!(PreviewScenario::parse(Some("unsupported")).is_err());
}

#[test]
fn full_access_scenario_grants_every_mail_and_calendar_action() {
    let full = preview_capabilities(PreviewScenario::FullAccess);
    assert_eq!(full.mail(), MailCapabilities::full_access());
    assert_eq!(full.calendar(), CalendarCapabilities::full_access());

    let standard = preview_capabilities(PreviewScenario::Standard);
    assert!(standard.mail().is_read_only());
    assert!(standard.calendar().is_read_only());
}

#[tokio::test]
async fn recovery_scenario_fails_once_then_returns_a_page() {
    let account_id = synthetic_account_id("preview-account");
    let provider = PreviewMailProvider::new(account_id.clone(), PreviewScenario::MailSyncRecovery);
    let folder = folder_reference(&account_id, "inbox");
    let foreign_folder = folder_reference(&synthetic_account_id("other-account"), "inbox");

    assert_eq!(
        provider
            .message_delta_page(&foreign_folder, MessageDeltaRequest::Initial)
            .await,
        Err(MailOperationError::AccountMismatch)
    );
    assert_eq!(
        provider
            .message_delta_page(&folder, MessageDeltaRequest::Initial)
            .await,
        Err(MailOperationError::ServiceUnavailable)
    );
    assert!(matches!(
        provider
            .message_delta_page(&folder, MessageDeltaRequest::Initial)
            .await
            .expect("retry succeeds after the synthetic failure"),
        MessageDeltaResponse::Page(_)
    ));
}

#[tokio::test]
async fn standard_scenario_does_not_inject_a_mail_sync_failure() {
    let account_id = synthetic_account_id("preview-account");
    let provider = PreviewMailProvider::new(account_id.clone(), PreviewScenario::Standard);
    let folder = folder_reference(&account_id, "inbox");

    assert!(matches!(
        provider
            .message_delta_page(&folder, MessageDeltaRequest::Initial)
            .await
            .expect("standard preview synchronization succeeds"),
        MessageDeltaResponse::Page(_)
    ));
}

fn synthetic_account_id(value: &str) -> AccountId {
    AccountId::try_from(value.to_owned()).expect("synthetic account ID is valid")
}

mod ui;
