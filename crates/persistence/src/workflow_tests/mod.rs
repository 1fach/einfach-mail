use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use einfach_mail_application::{
    AccountCache, AccountCacheRemovalError, AccountSessionFactory, AccountSessionWorkflowFactory,
    CalendarOperationError, CalendarWorkflowError, MailCache, MailCacheError, MailOperationError,
    MailSendOutcome, MailSendProviderError, MailSendResolutionError, MailSendWorkflowError,
    MailWorkflow, MailWorkflowError, MessageBodyForDisplay, MessageDeltaPageResponse,
    MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, FolderId, FolderReference, FollowUpFlagStatus, MailReplyDraft, MailReplySource,
    MailSendOperationId, MessageBody, MessageBodyType, MessageDeltaChange, MessageDeltaLink,
    MessageDeltaPage, MessageDeltaProgress, MessageId, ReadState,
};
use tempfile::TempDir;

use crate::store::SqliteAccountCache;

mod account_lifecycle;
mod mail_actions;
mod mail_sync;

#[path = "../test_support.rs"]
mod test_support;
use test_support::*;
