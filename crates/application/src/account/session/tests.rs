use super::AccountSessionWorkflowFactory;
use einfach_mail_domain::AccountId;

fn account_id(value: &str) -> AccountId {
    AccountId::try_from(value.to_owned()).expect("synthetic account ID is valid")
}

#[test]
fn workflow_recovery_is_shared_per_account_and_restarts_after_last_session_drops() {
    let factory = AccountSessionWorkflowFactory::new((), ());
    let account = account_id("synthetic-account");
    let first = factory
        .recovery_for_account(&account)
        .expect("account recovery state is available");
    first.mark_mail_sync_ready();

    let second = factory
        .recovery_for_account(&account)
        .expect("account recovery state is available");
    assert!(second.mail_sync_ready());

    let other_account = factory
        .recovery_for_account(&account_id("other-account"))
        .expect("other account recovery state is available");
    assert!(!other_account.mail_sync_ready());

    drop(first);
    drop(second);
    let reopened = factory
        .recovery_for_account(&account)
        .expect("new account recovery state is available");
    assert!(!reopened.mail_sync_ready());
}
