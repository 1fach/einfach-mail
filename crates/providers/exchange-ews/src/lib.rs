//! Read-only Exchange Server mail access through EWS.

mod autodiscover;
mod error;
mod ntlm;
mod provider;
mod transport;

pub use provider::ExchangeEwsProvider;
