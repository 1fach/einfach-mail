//! User workflows and ports for mail, calendar, identity, and local storage.
//!
//! This crate coordinates domain behavior without depending on a UI framework, Microsoft Graph, or a database.

mod account;
mod account_cache;
mod account_session;
mod calendar;
mod calendar_cache;
mod calendar_provider;
mod email_html;
mod exchange_ews_settings;
mod mail;
mod mail_cache;
mod mail_workflow;
mod shared;

use shared::recovery as account_recovery;

pub use account::{
    AccountConnectionRequest, AccountOperationCancellation, AccountOperationError,
    AccountPermissionRequest, AccountProvider, AccountWorkflow, ExchangeEwsConnectionInput,
    ExchangeEwsDiscoveryConfirmation, ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult,
};
pub use account_cache::{AccountCache, AccountCacheOpenError, AccountCacheRemovalError};
pub use account_recovery::CalendarWriteReadiness;
pub use account_session::{
    AccountCacheFactory, AccountSession, AccountSessionFactory, AccountSessionFuture,
    AccountSessionWorkflowFactory, CalendarSession, MailSession,
};
pub use calendar::{
    CalendarEventCreationOutcome, CalendarEventDeletionOutcome, CalendarEventUpdateOutcome,
    CalendarSnapshot, CalendarSyncReport, CalendarWorkflow, CalendarWorkflowError,
};
pub use calendar_cache::{CalendarCache, CalendarCacheError, CalendarSyncMode, CalendarSyncState};
pub use calendar_provider::{
    CalendarDeltaPageResponse, CalendarDeltaRequest, CalendarDeltaResponse, CalendarOperationError,
    CalendarProvider, CalendarSyncStrategy, CalendarViewSnapshotResponse,
};
pub use email_html::{
    ExternalMessageLink, MessageBodyForDisplay, SanitizedEmailHtml, allowed_external_message_url,
    prepare_message_body_for_display,
};
pub use exchange_ews_settings::{
    ExchangeEwsAccountSettings, ExchangeEwsAccountSettingsStore, ExchangeEwsSettingsError,
};
pub use mail::{
    MailOperationError, MailProvider, MailSendProviderError, MessageDeltaPageResponse,
    MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
pub use mail_cache::{MailCache, MailCacheError, MailSyncState};
pub use mail_workflow::{
    FollowUpFlagUpdateOutcome, MailSendOutcome, MailSendResolutionError, MailSendWorkflowError,
    MailSyncProgress, MailSyncReport, MailWorkflow, MailWorkflowError, MessageMoveOutcome,
    OpenedMessage, ReadStateUpdateOutcome, WellKnownFolderMoveOutcome,
};
