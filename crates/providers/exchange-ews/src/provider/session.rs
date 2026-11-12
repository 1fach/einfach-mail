use std::sync::Arc;

use einfach_mail_application::{
    AccountOperationError, CalendarOperationError, ExchangeEwsAccountSettings,
    ExchangeEwsConnectionInput, MailOperationError,
};
use einfach_mail_domain::{
    AccountCapabilities, AccountId, AccountProfile, CalendarCapabilities, MailAction,
    MailCapabilities,
};
use secrecy::SecretString;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::{
    auth::MAX_CREDENTIAL_BYTES, error::EwsError, transport::EwsTransport,
    wire::ExchangeProductVersion,
};

pub(super) struct EwsSession {
    pub(super) profile: AccountProfile,
    pub(super) transport: EwsTransport,
}

pub(super) async fn session_for_mail(
    sessions: &RwLock<Option<Arc<EwsSession>>>,
    account_id: &AccountId,
) -> Result<Arc<EwsSession>, MailOperationError> {
    let session = sessions
        .read()
        .await
        .clone()
        .ok_or(MailOperationError::SignInRequired)?;
    if session.profile.id() != account_id {
        return Err(MailOperationError::AccountMismatch);
    }
    Ok(session)
}

pub(super) async fn session_for_calendar(
    sessions: &RwLock<Option<Arc<EwsSession>>>,
    account_id: &AccountId,
) -> Result<Arc<EwsSession>, CalendarOperationError> {
    let session = sessions
        .read()
        .await
        .clone()
        .ok_or(CalendarOperationError::SignInRequired)?;
    if session.profile.id() != account_id {
        return Err(CalendarOperationError::AccountMismatch);
    }
    Ok(session)
}

impl EwsSession {
    pub(super) fn new(
        settings: ExchangeEwsAccountSettings,
        password: SecretString,
    ) -> Result<Self, EwsError> {
        let profile =
            profile_for(&settings, None).map_err(|_| EwsError::InvalidConnectionDetails)?;
        let transport = EwsTransport::new(
            settings.endpoint(),
            settings.username().to_owned(),
            password,
        )?;
        Ok(Self { profile, transport })
    }
}

pub(super) fn validate_settings(
    input: &ExchangeEwsConnectionInput,
) -> Result<ExchangeEwsAccountSettings, AccountOperationError> {
    let endpoint = input.endpoint().trim();
    let mailbox = input.mailbox_address().trim();
    let username = input.username().trim();
    let password = input.expose_password();
    if endpoint.is_empty()
        || endpoint.len() > 2 * 1024
        || mailbox.is_empty()
        || mailbox.len() > 512
        || username.is_empty()
        || username.len() > 512
        || password.is_empty()
        || password.len() > MAX_CREDENTIAL_BYTES
        || [endpoint, mailbox, username, password]
            .iter()
            .any(|value| value.chars().any(char::is_control))
    {
        return Err(AccountOperationError::InvalidConnectionDetails);
    }
    let settings = ExchangeEwsAccountSettings::new(
        endpoint.to_owned(),
        mailbox.to_owned(),
        username.to_owned(),
    );
    profile_for(&settings, None).map_err(|_| AccountOperationError::InvalidConnectionDetails)?;
    Ok(settings)
}

pub(super) fn profile_for(
    settings: &ExchangeEwsAccountSettings,
    server_version: Option<ExchangeProductVersion>,
) -> Result<AccountProfile, EwsError> {
    let parsed_endpoint =
        url::Url::parse(settings.endpoint()).map_err(|_| EwsError::InvalidConnectionDetails)?;
    if parsed_endpoint.scheme() != "https"
        || parsed_endpoint.host_str().is_none()
        || !parsed_endpoint.username().is_empty()
        || parsed_endpoint.password().is_some()
        || parsed_endpoint.query().is_some()
        || parsed_endpoint.fragment().is_some()
    {
        return Err(EwsError::InvalidConnectionDetails);
    }
    let mailbox_address = settings.mailbox_address().trim();
    let account_key = format!(
        "{}\n{}",
        parsed_endpoint.as_str(),
        mailbox_address.to_ascii_lowercase()
    );
    let id = AccountId::try_from(format!("ews:{}", hex::encode(Sha256::digest(account_key))))
        .map_err(|_| EwsError::InvalidConnectionDetails)?;
    let mail_capabilities =
        if server_version.is_some_and(ExchangeProductVersion::supports_read_receipt_suppression) {
            MailCapabilities::from_actions([MailAction::SetReadState])
        } else {
            MailCapabilities::read_only()
        };
    AccountProfile::try_new_with_capabilities(
        id,
        Some(mailbox_address.to_owned()),
        Some(mailbox_address.to_owned()),
        Some(settings.username().to_owned()),
        AccountCapabilities::new(mail_capabilities, CalendarCapabilities::view_only()),
    )
    .map_err(|_| EwsError::InvalidConnectionDetails)
}
