//! Shared state and configuration for the Microsoft Graph provider.

use std::sync::{Arc, OnceLock};

use huskarl::token::AccessToken;
use uuid::Uuid;

use crate::{
    CallbackPort,
    auth::{
        credential::{RefreshTokenStore, SecretServiceRefreshTokenStore},
        identity::{BrowserLauncher, Endpoints, SystemBrowser},
    },
    error::GraphError,
    transport::GraphAuthority,
    transport::GraphTransport,
    wire::GraphApi,
};
use einfach_mail_domain::AccountId;

mod account;
mod calendar;
mod mail;
mod session;

#[cfg(test)]
mod tests;

/// Microsoft Graph account and mail provider for the Linux desktop application.
#[derive(Clone)]
pub struct MicrosoftGraphProvider {
    config: Arc<ProviderConfig>,
}

struct ProviderConfig {
    client_id: Option<Uuid>,
    callback_port: CallbackPort,
    endpoints: Endpoints,
    browser: Arc<dyn BrowserLauncher>,
    credentials: Arc<dyn RefreshTokenStore>,
    graph_authority: GraphAuthority,
    graph_transport: OnceLock<Result<GraphTransport, GraphError>>,
    session: tokio::sync::Mutex<Option<ProviderSession>>,
}

struct ProviderSession {
    account_id: AccountId,
    access_token: Option<Arc<AccessToken>>,
}

impl MicrosoftGraphProvider {
    /// Creates a public-client provider for the `organizations` authority.
    ///
    /// The app ID must belong to a public client registration. The provider
    /// never accepts or stores a client secret.
    #[must_use]
    pub fn new(client_id: Uuid, callback_port: CallbackPort) -> Self {
        Self::with_client_id(Some(client_id), callback_port)
    }

    /// Creates a provider that can inspect and remove saved credentials without Microsoft sign-in.
    #[must_use]
    pub fn new_unconfigured() -> Self {
        Self::with_client_id(None, CallbackPort::Ephemeral)
    }

    fn with_client_id(client_id: Option<Uuid>, callback_port: CallbackPort) -> Self {
        let endpoints = Endpoints::microsoft();
        let graph_authority = GraphAuthority::from_profile_endpoint(&endpoints.graph_me)
            .expect("the static Microsoft Graph profile URL is valid");
        Self {
            config: Arc::new(ProviderConfig {
                client_id,
                callback_port,
                endpoints,
                browser: Arc::new(SystemBrowser),
                credentials: Arc::new(SecretServiceRefreshTokenStore::new()),
                graph_authority,
                graph_transport: OnceLock::new(),
                session: tokio::sync::Mutex::new(None),
            }),
        }
    }
}

impl ProviderConfig {
    fn graph_api(&self) -> Result<GraphApi, GraphError> {
        let transport = self
            .graph_transport
            .get_or_init(GraphTransport::start)
            .as_ref()
            .map_err(|error| *error)?;
        Ok(GraphApi::new(
            self.graph_authority.clone(),
            transport.handle(),
        ))
    }
}
