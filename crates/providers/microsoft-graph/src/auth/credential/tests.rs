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
    AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is non-empty")
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
    let secret = super::encode_credential(&identity, &token).expect("versioned token serializes");

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
