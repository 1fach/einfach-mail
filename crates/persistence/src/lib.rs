//! SQLCipher-backed mail and calendar cache storage for Linux.
//!
//! The application layer uses [`SqliteAccountCache`] through its provider-independent
//! cache port and never receives a database or encryption-key handle.

#[cfg(all(test, target_os = "linux"))]
mod calendar_tests;
#[cfg(target_os = "linux")]
mod schema;
#[cfg(target_os = "linux")]
mod security;
#[cfg(target_os = "linux")]
mod store;
#[cfg(all(test, target_os = "linux"))]
mod workflow_tests;

#[cfg(target_os = "linux")]
pub use store::{SqliteAccountCache, SqliteAccountCacheFactory};
