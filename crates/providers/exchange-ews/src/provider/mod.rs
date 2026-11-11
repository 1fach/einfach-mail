use std::sync::Arc;

use einfach_mail_application::ExchangeEwsAccountSettingsStore;
use tokio::sync::RwLock;

use self::{account::EwsAutodiscover, session::EwsSession};

mod account;
mod calendar;
mod mail;
mod session;
pub(crate) use self::mail::read_state;
#[cfg(test)]
mod tests;

/// Connects to one on-premises Exchange account through Linux EWS and NTLMv2.
#[derive(Clone)]
pub struct ExchangeEwsProvider<S> {
    settings_store: Arc<S>,
    autodiscover: Arc<EwsAutodiscover>,
    session: Arc<RwLock<Option<Arc<EwsSession>>>>,
}

impl<S> ExchangeEwsProvider<S>
where
    S: ExchangeEwsAccountSettingsStore + 'static,
{
    /// Creates an EWS provider using the application's non-secret settings store.
    #[must_use]
    pub fn new(settings_store: S) -> Self {
        Self {
            settings_store: Arc::new(settings_store),
            autodiscover: Arc::new(EwsAutodiscover::default()),
            session: Arc::new(RwLock::new(None)),
        }
    }
}
