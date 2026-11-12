use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use httpmock::MockServer;
use huskarl::{
    core::secrets::SecretString,
    token::{AccessToken, BearerAccessToken},
};

pub(super) fn synthetic_access_token(token_text: &str) -> Arc<AccessToken> {
    Arc::new(AccessToken::Bearer(BearerAccessToken::new(
        SecretString::new(token_text),
        SystemTime::now(),
        Some(Duration::from_secs(3600)),
    )))
}

pub(super) fn http_url(server: &MockServer, path: &str) -> String {
    format!("http://{}{path}", server.address())
}

pub(super) fn http_base_url(server: &MockServer) -> String {
    http_url(server, "")
}
