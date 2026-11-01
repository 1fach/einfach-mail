//! User workflows and ports for mail, calendar, identity, and local storage.
//!
//! This crate coordinates domain behavior without depending on a UI framework, Microsoft Graph, or a database.

mod account;
mod mail;
mod mail_cache;
mod mail_workflow;

pub use account::{
    AccountOperationCancellation, AccountOperationError, AccountProvider, AccountWorkflow,
};
