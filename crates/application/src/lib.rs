//! User workflows and ports for mail, calendar, identity, and local storage.
//!
//! This crate coordinates domain behavior without depending on a UI framework, Microsoft Graph, or a database.

mod account;
mod account_session;
mod calendar;
mod email_html;
mod mail;
mod mail_cache;
mod mail_workflow;

pub use account::{
    AccountOperationCancellation, AccountOperationError, AccountProvider, AccountWorkflow,
};
pub use account_session::{
    AccountCacheFactory, AccountSession, AccountSessionFactory, AccountSessionFuture,
    CalendarSession, MailSession,
};
pub use calendar::{
    CalendarCache, CalendarCacheError, CalendarDeltaPageResponse, CalendarDeltaRequest,
    CalendarOperationError, CalendarProvider, CalendarSnapshot, CalendarSyncReport,
    CalendarSyncState, CalendarWorkflow, CalendarWorkflowError,
};
pub use email_html::{
    ExternalMessageLink, MessageBodyForDisplay, SanitizedEmailHtml, allowed_external_message_url,
    prepare_message_body_for_display,
};
pub use mail::{
    MailOperationError, MailProvider, MessageDeltaPageResponse, MessageDeltaRequest,
    MessageDeltaResponse,
};
pub use mail_cache::{MailCache, MailCacheError, MailSyncState};
pub use mail_workflow::{
    AccountSessionWorkflowFactory, MailSyncProgress, MailSyncReport, MailWorkflow,
    MailWorkflowError, OpenedMessage,
};
