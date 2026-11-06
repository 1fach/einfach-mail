//! Microsoft Graph API v1.0.
//!
//! `rest` mirrors the Graph resource tree (`users.*`); `send` is the
//! HTTP/JSON transport every coroutine delegates to, and `query` turns a
//! params struct into OData query pairs.

#[cfg(feature = "client")]
pub mod client;
pub mod field;
pub mod query;
pub mod rest;
pub mod send;
