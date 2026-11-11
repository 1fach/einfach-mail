use einfach_mail_domain::{AccountCapabilities, AccountId, AccountProfile, MissingAccountLabel};

pub(crate) fn synthetic_account_id() -> AccountId {
    AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid")
}

pub(crate) fn synthetic_account_profile(
    account_id: AccountId,
    user_principal_name: Option<String>,
    capabilities: AccountCapabilities,
) -> Result<AccountProfile, MissingAccountLabel> {
    AccountProfile::try_new_with_capabilities(
        account_id,
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        user_principal_name,
        capabilities,
    )
}
