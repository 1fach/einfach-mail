//! Exchange Server mail access through EWS on Linux.

mod autodiscover;
mod calendar;
mod error;
mod ntlm;
mod provider;
mod read_state;
mod transport;
mod version;

pub use provider::ExchangeEwsProvider;
