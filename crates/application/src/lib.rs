//! User workflows and ports for mail, calendar, identity, and local storage.
//!
//! This crate coordinates domain behavior without depending on a UI framework, Microsoft Graph, or a database.

mod account;
mod calendar;
mod mail;
mod shared;
#[cfg(test)]
mod test_support;

pub use account::{
    AccountCache, AccountCacheFactory, AccountCacheOpenError, AccountCacheRemovalError,
    AccountConnectionRequest, AccountOperationCancellation, AccountOperationError,
    AccountPermissionRequest, AccountProvider, AccountProviderKind, AccountSession,
    AccountSessionFactory, AccountSessionFuture, AccountSessionWorkflowFactory, AccountWorkflow,
    CalendarSession, ConnectedAccount, ExchangeEwsAccountSettings, ExchangeEwsAccountSettingsStore,
    ExchangeEwsConnectionInput, ExchangeEwsDiscoveryConfirmation, ExchangeEwsDiscoveryRequest,
    ExchangeEwsDiscoveryResult, ExchangeEwsSettingsError, MailSession,
};
pub use calendar::{
    CalendarCache, CalendarCacheError, CalendarDeltaPageResponse, CalendarDeltaRequest,
    CalendarDeltaResponse, CalendarEventCreationOutcome, CalendarEventDeletionOutcome,
    CalendarEventUpdateOutcome, CalendarOperationError, CalendarProvider, CalendarSnapshot,
    CalendarSyncMode, CalendarSyncReport, CalendarSyncState, CalendarSyncStrategy,
    CalendarViewSnapshotResponse, CalendarWorkflow, CalendarWorkflowError,
};
pub use mail::{
    ExternalMessageLink, FollowUpFlagUpdateOutcome, MailCache, MailCacheError, MailOperationError,
    MailProvider, MailSendOutcome, MailSendProviderError, MailSendResolutionError,
    MailSendWorkflowError, MailSyncProgress, MailSyncReport, MailSyncState, MailWorkflow,
    MailWorkflowError, MessageBodyForDisplay, MessageDeltaPageResponse, MessageDeltaRequest,
    MessageDeltaResponse, MessageMoveOutcome, OpenedMessage, ReadStateUpdateOutcome,
    SanitizedEmailHtml, WellKnownFolderMoveOutcome, WellKnownMailFolder,
    allowed_external_message_url, prepare_message_body_for_display,
};
pub use shared::recovery::CalendarWriteReadiness;
