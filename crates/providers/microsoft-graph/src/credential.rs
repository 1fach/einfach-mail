//! Refresh-token persistence through Linux Secret Service.

use huskarl::token::RefreshToken;
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use einfach_mail_application::AccountOperationError;
use einfach_mail_domain::AccountId;

const CREDENTIAL_SERVICE: &str = "com.1fach.einfach-mail";
const CREDENTIAL_ACCOUNT: &str = "microsoft-365-refresh-token";

pub(super) trait RefreshTokenStore: Send + Sync {
    fn load(&self) -> Result<Option<SavedRefreshToken>, AccountOperationError>;
    fn save(
        &self,
        identity: &CredentialIdentity,
        token: &RefreshToken,
    ) -> Result<(), AccountOperationError>;
    fn delete(&self) -> Result<(), AccountOperationError>;
}

/// Records whether Graph has verified the account associated with a refresh token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CredentialIdentity {
    /// Graph `/me` confirmed the account ID.
    Verified(AccountId),
    /// Graph has not confirmed the account ID.
    ///
    /// A prior ID remains available as an expected value for the next restore.
    Unverified {
        expected_account_id: Option<AccountId>,
    },
}

impl CredentialIdentity {
    fn verified_account_id(&self) -> Option<&AccountId> {
        match self {
            Self::Verified(account_id) => Some(account_id),
            Self::Unverified { .. } => None,
        }
    }

    fn expected_account_id(&self) -> Option<&AccountId> {
        match self {
            Self::Verified(account_id) => Some(account_id),
            Self::Unverified {
                expected_account_id,
            } => expected_account_id.as_ref(),
        }
    }
}

/// Keeps a refresh token beside the identity state that constrains its use.
#[derive(Clone)]
pub(super) struct SavedRefreshToken {
    identity: CredentialIdentity,
    token: RefreshToken,
}

impl SavedRefreshToken {
    pub(super) fn with_identity(identity: CredentialIdentity, token: RefreshToken) -> Self {
        Self { identity, token }
    }

    /// Returns an account ID only after Graph has verified it.
    pub(super) fn account_id(&self) -> Option<&AccountId> {
        self.identity.verified_account_id()
    }

    /// Returns the account ID that a restored Graph profile must match, if known.
    pub(super) fn expected_account_id(&self) -> Option<&AccountId> {
        self.identity.expected_account_id()
    }

    pub(super) fn token(&self) -> &RefreshToken {
        &self.token
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedCredential {
    version: u8,
    account_id: Option<String>,
    #[serde(default)]
    expected_account_id: Option<String>,
    refresh_token: RefreshToken,
}

#[derive(Serialize)]
struct EncodedCredential<'a> {
    version: u8,
    account_id: Option<&'a str>,
    expected_account_id: Option<&'a str>,
    refresh_token: &'a RefreshToken,
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

    /// Creates an isolated store for Secret Service integration tests.
    #[cfg(test)]
    pub(super) fn with_account(account: String) -> Self {
        Self { account }
    }

    fn entry(&self) -> Result<Entry, AccountOperationError> {
        Entry::new(CREDENTIAL_SERVICE, &self.account)
            .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
    }
}

/// Removes a synthetic credential from Secret Service when the test ends.
#[cfg(test)]
pub(super) struct SecretServiceCredentialCleanup(SecretServiceRefreshTokenStore);

#[cfg(test)]
impl SecretServiceCredentialCleanup {
    /// Retains the store so its credential can be removed when the guard drops.
    pub(super) fn new(store: SecretServiceRefreshTokenStore) -> Self {
        Self(store)
    }
}

#[cfg(test)]
impl Drop for SecretServiceCredentialCleanup {
    fn drop(&mut self) {
        if let Err(error) = self.0.delete() {
            eprintln!("could not remove synthetic provider credential: {error}");
        }
    }
}

impl RefreshTokenStore for SecretServiceRefreshTokenStore {
    fn load(&self) -> Result<Option<SavedRefreshToken>, AccountOperationError> {
        let secret = match self.entry()?.get_secret() {
            Ok(secret) => Zeroizing::new(secret),
            Err(KeyringError::NoEntry) => return Ok(None),
            Err(_) => return Err(AccountOperationError::CredentialStoreUnavailable),
        };

        decode_credential(&secret).map(Some)
    }

    fn save(
        &self,
        identity: &CredentialIdentity,
        token: &RefreshToken,
    ) -> Result<(), AccountOperationError> {
        let secret = encode_credential(identity, token)?;
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

#[derive(Deserialize)]
struct CredentialShape {
    #[serde(default, deserialize_with = "mark_field_present")]
    version: bool,
    #[serde(default, deserialize_with = "mark_field_present")]
    account_id: bool,
    #[serde(default, deserialize_with = "mark_field_present")]
    expected_account_id: bool,
    #[serde(default, deserialize_with = "mark_field_present")]
    refresh_token: bool,
}

fn mark_field_present<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::de::IgnoredAny::deserialize(deserializer).map(|_| true)
}

fn decode_credential(secret: &[u8]) -> Result<SavedRefreshToken, AccountOperationError> {
    let shape: CredentialShape = serde_json::from_slice(secret)
        .map_err(|_| AccountOperationError::CredentialStoreUnavailable)?;
    if shape.version || shape.account_id || shape.expected_account_id || shape.refresh_token {
        let credential = serde_json::from_slice(secret)
            .map_err(|_| AccountOperationError::CredentialStoreUnavailable)?;
        return decode_versioned_credential(credential);
    }
    let token = serde_json::from_slice(secret)
        .map_err(|_| AccountOperationError::CredentialStoreUnavailable)?;
    Ok(SavedRefreshToken::with_identity(
        CredentialIdentity::Unverified {
            expected_account_id: None,
        },
        token,
    ))
}

fn decode_versioned_credential(
    credential: VersionedCredential,
) -> Result<SavedRefreshToken, AccountOperationError> {
    let VersionedCredential {
        version,
        account_id,
        expected_account_id,
        refresh_token,
    } = credential;
    let identity = decode_credential_identity(version, account_id, expected_account_id)?;
    Ok(SavedRefreshToken::with_identity(identity, refresh_token))
}

fn decode_credential_identity(
    version: u8,
    account_id: Option<String>,
    expected_account_id: Option<String>,
) -> Result<CredentialIdentity, AccountOperationError> {
    if !matches!(version, 1 | 2) || (version == 1 && expected_account_id.is_some()) {
        return Err(AccountOperationError::CredentialStoreUnavailable);
    }

    let account_id = parse_account_id(account_id)?;
    let expected_account_id = parse_account_id(expected_account_id)?;
    match (version, account_id, expected_account_id) {
        (1, Some(account_id), None) => Ok(CredentialIdentity::Unverified {
            expected_account_id: Some(account_id),
        }),
        (2, Some(account_id), None) => Ok(CredentialIdentity::Verified(account_id)),
        (_, None, expected_account_id) => Ok(CredentialIdentity::Unverified {
            expected_account_id,
        }),
        (_, Some(_), Some(_)) => Err(AccountOperationError::CredentialStoreUnavailable),
        _ => Err(AccountOperationError::CredentialStoreUnavailable),
    }
}

fn parse_account_id(value: Option<String>) -> Result<Option<AccountId>, AccountOperationError> {
    value
        .map(AccountId::try_from)
        .transpose()
        .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
}

fn encode_credential(
    identity: &CredentialIdentity,
    token: &RefreshToken,
) -> Result<Zeroizing<Vec<u8>>, AccountOperationError> {
    let (account_id, expected_account_id) = match identity {
        CredentialIdentity::Verified(account_id) => (Some(account_id.as_str()), None),
        CredentialIdentity::Unverified {
            expected_account_id,
        } => (None, expected_account_id.as_ref().map(AccountId::as_str)),
    };
    let credential = EncodedCredential {
        version: 2,
        account_id,
        expected_account_id,
        refresh_token: token,
    };
    serde_json::to_vec(&credential)
        .map(Zeroizing::new)
        .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use einfach_mail_domain::AccountId;
    use huskarl::token::RefreshToken;

    use super::{
        CredentialIdentity, RefreshTokenStore, SecretServiceCredentialCleanup,
        SecretServiceRefreshTokenStore,
    };

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
        let _cleanup = SecretServiceCredentialCleanup::new(store.clone());
        let original_token = synthetic_refresh_token("synthetic-refresh-token-original");
        let rotated_token = synthetic_refresh_token("synthetic-refresh-token-rotated");
        let account_id = synthetic_account_id();
        let identity = CredentialIdentity::Verified(account_id.clone());

        assert!(store.load().expect("credential lookup succeeds").is_none());
        store
            .save(&identity, &original_token)
            .expect("synthetic refresh token is stored");
        assert_stored_credential(&store, &account_id, &original_token);

        store
            .save(&identity, &rotated_token)
            .expect("rotated refresh token replaces the original");
        assert_stored_credential(&store, &account_id, &rotated_token);

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

    fn synthetic_account_id() -> AccountId {
        AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is non-empty")
    }

    fn assert_stored_credential(
        store: &impl RefreshTokenStore,
        expected_account_id: &AccountId,
        expected_token: &RefreshToken,
    ) {
        let actual = store
            .load()
            .expect("credential lookup succeeds")
            .expect("refresh token is present");
        let actual_bytes = serde_json::to_vec(actual.token()).expect("stored token serializes");
        let expected_bytes =
            serde_json::to_vec(expected_token).expect("synthetic refresh token serializes");

        assert_eq!(actual.account_id(), Some(expected_account_id));
        assert_eq!(actual_bytes, expected_bytes);
    }

    #[test]
    fn reads_legacy_refresh_tokens_without_account_identity() {
        let token = synthetic_refresh_token("synthetic-refresh-token");
        let secret = serde_json::to_vec(&token).expect("synthetic token serializes");

        let saved = super::decode_credential(&secret).expect("legacy token is readable");

        assert_eq!(
            saved.identity,
            CredentialIdentity::Unverified {
                expected_account_id: None,
            }
        );
        assert!(saved.account_id().is_none());
        assert!(saved.expected_account_id().is_none());
        assert_eq!(
            serde_json::to_vec(saved.token()).expect("token serializes"),
            secret
        );
    }

    #[test]
    fn reads_verified_version_two_identity() {
        let account_id = synthetic_account_id();
        let token = synthetic_refresh_token("synthetic-refresh-token");
        let identity = CredentialIdentity::Verified(account_id.clone());
        let secret =
            super::encode_credential(&identity, &token).expect("versioned token serializes");

        let saved = super::decode_credential(&secret).expect("versioned token is readable");

        assert_eq!(saved.account_id(), Some(&account_id));
        assert_eq!(saved.expected_account_id(), Some(&account_id));
        assert_eq!(saved.token(), &token);
    }

    #[test]
    fn reads_pending_identity_with_its_expected_account() {
        let token = synthetic_refresh_token("synthetic-refresh-token");
        let expected_account_id = synthetic_account_id();
        let identity = CredentialIdentity::Unverified {
            expected_account_id: Some(expected_account_id.clone()),
        };
        let secret =
            super::encode_credential(&identity, &token).expect("pending credential serializes");

        let saved = super::decode_credential(&secret).expect("pending credential is readable");

        assert_eq!(
            saved.identity,
            CredentialIdentity::Unverified {
                expected_account_id: Some(expected_account_id.clone()),
            }
        );
        assert!(saved.account_id().is_none());
        assert_eq!(saved.expected_account_id(), Some(&expected_account_id));
    }

    #[test]
    fn reads_version_one_identity_as_unverified() {
        let account_id = synthetic_account_id();
        let secret = br#"{"version":1,"account_id":"synthetic-account","refresh_token":{"token":"synthetic-refresh-token"}}"#;

        let saved = super::decode_credential(secret).expect("version 1 credential is readable");

        assert_eq!(
            saved.identity,
            CredentialIdentity::Unverified {
                expected_account_id: Some(account_id.clone()),
            }
        );
        assert!(saved.account_id().is_none());
        assert_eq!(saved.expected_account_id(), Some(&account_id));
    }

    #[test]
    fn rejects_unknown_versions_and_invalid_identity_fields() {
        for secret in [
            br#"{"version":3,"account_id":"synthetic-account","expected_account_id":null,"refresh_token":{"token":"synthetic"}}"#.as_slice(),
            br#"{"version":2,"account_id":"","expected_account_id":null,"refresh_token":{"token":"synthetic"}}"#.as_slice(),
            br#"{"version":2,"account_id":"synthetic-account","expected_account_id":"other-account","refresh_token":{"token":"synthetic"}}"#.as_slice(),
            br#"{"version":1,"account_id":null,"expected_account_id":"synthetic-account","refresh_token":{"token":"synthetic"}}"#.as_slice(),
            br#"{"version":3,"account_id":"synthetic-account","refresh_token":{"token":"synthetic-versioned"},"token":"synthetic-legacy"}"#.as_slice(),
            br#"{"account_id":"synthetic-account","token":"synthetic-legacy"}"#.as_slice(),
        ] {
            assert!(matches!(
                super::decode_credential(secret),
                Err(super::AccountOperationError::CredentialStoreUnavailable)
            ));
        }
    }
}
