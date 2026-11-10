//! Microsoft Graph account connection adapter for Linux.

mod auth;
mod config;
mod error;
mod provider;
mod transport;
mod wire;

#[cfg(test)]
mod test_support;

pub use config::{CallbackPort, InvalidCallbackPort};
pub use provider::MicrosoftGraphProvider;
