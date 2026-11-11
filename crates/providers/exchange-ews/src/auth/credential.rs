use keyring::{Entry, Error as KeyringError};
use secrecy::SecretString;
use zeroize::{Zeroize, Zeroizing};

use crate::error::EwsError;

const CREDENTIAL_SERVICE: &str = "com.1fach.einfach-mail";
const CREDENTIAL_ACCOUNT: &str = "exchange-ews-password";
pub(crate) const MAX_CREDENTIAL_BYTES: usize = 4096;

fn keyring_entry() -> Result<Entry, EwsError> {
    Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
        .map_err(|_| EwsError::CredentialStoreUnavailable)
}

pub(crate) fn load_password() -> Result<Option<SecretString>, EwsError> {
    let mut secret = match keyring_entry()?.get_secret() {
        Ok(secret) => Zeroizing::new(secret),
        Err(KeyringError::NoEntry) => return Ok(None),
        Err(_) => return Err(EwsError::CredentialStoreUnavailable),
    };
    if secret.is_empty() || secret.len() > MAX_CREDENTIAL_BYTES {
        return Err(EwsError::CredentialStoreUnavailable);
    }
    let password_bytes = std::mem::take(&mut *secret);
    Ok(Some(decode_password(password_bytes)?))
}

pub(crate) fn decode_password(password_bytes: Vec<u8>) -> Result<SecretString, EwsError> {
    match String::from_utf8(password_bytes) {
        Ok(password) => Ok(SecretString::from(password)),
        Err(error) => {
            let mut password_bytes = error.into_bytes();
            password_bytes.zeroize();
            Err(EwsError::CredentialStoreUnavailable)
        }
    }
}

pub(crate) fn save_password(password: &[u8]) -> Result<(), EwsError> {
    if password.is_empty() || password.len() > MAX_CREDENTIAL_BYTES {
        return Err(EwsError::InvalidConnectionDetails);
    }
    keyring_entry()?
        .set_secret(password)
        .map_err(|_| EwsError::CredentialStoreUnavailable)
}

pub(crate) fn delete_password() -> Result<(), EwsError> {
    match keyring_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Err(EwsError::CredentialStoreUnavailable),
    }
}
