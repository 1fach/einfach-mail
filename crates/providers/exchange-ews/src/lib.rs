//! Exchange Server mail access through EWS on Linux.

mod auth;
mod error;
mod provider;
mod transport;
mod wire;

pub use provider::ExchangeEwsProvider;
