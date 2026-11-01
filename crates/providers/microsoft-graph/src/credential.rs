//! Refresh-token persistence through Linux Secret Service.

use huskarl::token::RefreshToken;
use keyring::{Entry, Error as KeyringError};
use zeroize::Zeroizing;

use einfach_mail_application::AccountOperationError;

const CREDENTIAL_SERVICE: &str = "com.1fach.einfach-mail";
const CREDENTIAL_ACCOUNT: &str = "microsoft-365-refresh-token";

pub(super) trait RefreshTokenStore: Send + Sync {
    fn load(&self) -> Result<Option<RefreshToken>, AccountOperationError>;
    fn save(&self, token: &RefreshToken) -> Result<(), AccountOperationError>;
    fn delete(&self) -> Result<(), AccountOperationError>;
}

#[derive(Clone, Debug)]
pub(super) struct SecretServiceRefreshTokenStore {
    account: String,
}

impl SecretServiceRefreshTokenStore {
    pub(super) fn new() -> Self {
        Self {
            account: CREDENTIAL_ACCOUNT.to_owned(),
        }
    }

    #[cfg(test)]
    fn with_account(account: String) -> Self {
        Self { account }
    }

    fn entry(&self) -> Result<Entry, AccountOperationError> {
        Entry::new(CREDENTIAL_SERVICE, &self.account)
            .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
    }
}

impl RefreshTokenStore for SecretServiceRefreshTokenStore {
    fn load(&self) -> Result<Option<RefreshToken>, AccountOperationError> {
        let secret = match self.entry()?.get_secret() {
            Ok(secret) => Zeroizing::new(secret),
            Err(KeyringError::NoEntry) => return Ok(None),
            Err(_) => return Err(AccountOperationError::CredentialStoreUnavailable),
        };

        serde_json::from_slice(&secret)
            .map(Some)
            .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
    }

    fn save(&self, token: &RefreshToken) -> Result<(), AccountOperationError> {
        let secret = Zeroizing::new(
            serde_json::to_vec(token)
                .map_err(|_| AccountOperationError::CredentialStoreUnavailable)?,
        );
        self.entry()?
            .set_secret(secret.as_slice())
            .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
    }

    fn delete(&self) -> Result<(), AccountOperationError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(_) => Err(AccountOperationError::CredentialStoreUnavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use huskarl::token::RefreshToken;

    use super::{RefreshTokenStore, SecretServiceRefreshTokenStore};

    struct CredentialCleanup(SecretServiceRefreshTokenStore);

    impl Drop for CredentialCleanup {
        fn drop(&mut self) {
            if let Err(error) = self.0.delete() {
                eprintln!("could not remove synthetic provider credential: {error}");
            }
        }
    }

    #[test]
    #[ignore = "requires an unlocked Linux Secret Service session"]
    fn secret_service_store_saves_replaces_loads_and_removes_refresh_tokens() {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let store = SecretServiceRefreshTokenStore::with_account(format!(
            "provider-test-{}-{timestamp}",
            std::process::id(),
        ));
        let _cleanup = CredentialCleanup(store.clone());
        let original_token = synthetic_refresh_token("synthetic-refresh-token-original");
        let rotated_token = synthetic_refresh_token("synthetic-refresh-token-rotated");

        assert!(store.load().expect("credential lookup succeeds").is_none());
        store
            .save(&original_token)
            .expect("synthetic refresh token is stored");
        assert_stored_token(&store, &original_token);

        store
            .save(&rotated_token)
            .expect("rotated refresh token replaces the original");
        assert_stored_token(&store, &rotated_token);

        store.delete().expect("credential is removed");
        assert!(store.load().expect("credential lookup succeeds").is_none());
        store
            .delete()
            .expect("removing a missing credential succeeds");
    }

    fn synthetic_refresh_token(value: &str) -> RefreshToken {
        serde_json::from_str(&format!(r#"{{"token":"{value}"}}"#))
            .expect("synthetic refresh token is valid")
    }

    fn assert_stored_token(store: &impl RefreshTokenStore, expected: &RefreshToken) {
        let actual = store
            .load()
            .expect("credential lookup succeeds")
            .expect("refresh token is present");
        let actual_bytes = serde_json::to_vec(&actual).expect("stored refresh token serializes");
        let expected_bytes =
            serde_json::to_vec(expected).expect("synthetic refresh token serializes");

        assert_eq!(actual_bytes, expected_bytes);
    }
}
