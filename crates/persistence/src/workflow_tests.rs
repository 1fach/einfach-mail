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
    MessageDeltaRequest, MessageDeltaResponse,
};
use einfach_mail_domain::{
    AccountId, FolderId, FolderReference, FollowUpFlagStatus, MailFolderKind, MailReplyDraft,
    MailReplySource, MailSendOperationId, MessageBody, MessageBodyType, MessageDeltaChange,
    MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageId, ReadState,
};
use tempfile::TempDir;

use crate::SqliteAccountCache;
use crate::test_support::*;

mod account;
mod mail;
