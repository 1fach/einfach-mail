//! SQLCipher-backed storage for the Linux mail cache.
//!
//! The application layer uses [`SqliteMailCache`] through its provider-independent
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
pub use store::{SqliteMailCache, SqliteMailCacheFactory};
