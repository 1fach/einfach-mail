mod cache;
mod email_html;
mod provider;
mod workflow;

pub use cache::{MailCache, MailCacheError, MailSyncState};
pub use email_html::{
    ExternalMessageLink, MessageBodyForDisplay, SanitizedEmailHtml, allowed_external_message_url,
    prepare_message_body_for_display,
};
pub use provider::{
    MailOperationError, MailProvider, MailSendProviderError, MessageDeltaPageResponse,
    MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
pub use workflow::{
    FollowUpFlagUpdateOutcome, MailSendOutcome, MailSendResolutionError, MailSendWorkflowError,
    MailSyncProgress, MailSyncReport, MailWorkflow, MailWorkflowError, MessageMoveOutcome,
    OpenedMessage, ReadStateUpdateOutcome, WellKnownFolderMoveOutcome,
};
