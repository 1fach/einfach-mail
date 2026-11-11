//! SQLCipher-backed mail and calendar cache storage for Linux.
//!
//! The application layer uses [`SqliteAccountCache`] through its provider-independent
//! cache port and never receives a database or encryption-key handle.

#[cfg(all(test, target_os = "linux"))]
#[path = "calendar/tests/mod.rs"]
mod calendar_tests;
#[cfg(target_os = "linux")]
#[path = "account/ews_settings.rs"]
mod exchange_ews_settings;
#[cfg(target_os = "linux")]
#[path = "shared/schema/mod.rs"]
mod schema;
#[cfg(target_os = "linux")]
#[path = "platform/linux/security.rs"]
mod security;
#[cfg(target_os = "linux")]
#[path = "shared/store/mod.rs"]
mod store;
#[cfg(all(test, target_os = "linux"))]
#[path = "workflow_tests/mod.rs"]
mod workflow_tests;

#[cfg(target_os = "linux")]
pub use exchange_ews_settings::XdgExchangeEwsAccountSettingsStore;
#[cfg(target_os = "linux")]
pub use store::{SqliteAccountCache, SqliteAccountCacheFactory};
