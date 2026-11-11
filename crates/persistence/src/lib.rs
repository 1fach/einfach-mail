//! SQLCipher-backed mail and calendar cache storage for Linux.
//!
//! The application layer uses [`SqliteAccountCache`] through its provider-independent
//! cache port and never receives a database or encryption-key handle.

#[cfg(target_os = "linux")]
mod account;
#[cfg(target_os = "linux")]
mod calendar;
#[cfg(all(test, target_os = "linux"))]
#[path = "calendar/tests/mod.rs"]
mod calendar_tests;
#[cfg(target_os = "linux")]
mod mail;
#[cfg(target_os = "linux")]
mod platform;
#[cfg(target_os = "linux")]
mod shared;
#[cfg(all(test, target_os = "linux"))]
#[path = "workflow_tests/mod.rs"]
mod workflow_tests;

#[cfg(target_os = "linux")]
pub use account::XdgExchangeEwsAccountSettingsStore;
#[cfg(target_os = "linux")]
pub use shared::store::{SqliteAccountCache, SqliteAccountCacheFactory};
