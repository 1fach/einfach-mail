//! User workflows and ports for mail, calendar, identity, and local storage.
//!
//! This crate coordinates domain behavior without depending on a UI framework, Microsoft Graph, or a database.

mod account;
mod email_html;
mod mail;
mod mail_cache;
mod mail_workflow;

pub use account::{
    AccountOperationCancellation, AccountOperationError, AccountProvider, AccountWorkflow,
};
pub use email_html::{
    ExternalMessageLink, MessageBodyForDisplay, SanitizedEmailHtml, allowed_external_message_url,
    prepare_message_body_for_display,
};
pub use mail::{
    MailOperationError, MailProvider, MessageDeltaPageResponse, MessageDeltaRequest,
    MessageDeltaResponse,
};
pub use mail_cache::{MailCache, MailCacheError, MailCacheFactory, MailSyncState};
pub use mail_workflow::{
    MailSession, MailSessionFactory, MailSessionFuture, MailSyncProgress, MailSyncReport,
    MailWorkflow, MailWorkflowError, MailWorkflowFactory, OpenedMessage,
};
