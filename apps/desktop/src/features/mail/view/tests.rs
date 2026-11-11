use std::{
    future::{Future, pending, ready},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use einfach_mail_application::{
    AccountCache, AccountCacheRemovalError, AccountOperationError, MailCache, MailCacheError,
    MailOperationError, MailProvider, MailSendProviderError, MailSession, MailSyncState,
    MailWorkflow, MailWorkflowError, MessageBodyForDisplay, MessageDeltaPageResponse,
    MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountCapabilities, AccountId, AccountProfile, CalendarAccess, FolderId, FolderReference,
    FollowUpFlagStatus, MailAction, MailCapabilities, MailDraft, MailFolder, MailMessage,
    MailReplyDraft, MailSendOperationId, Mailbox, MessageBody, MessageBodyType, MessageDeltaLink,
    MessageDeltaPage, MessageDeltaProgress, MessageId, MessageMetadataPatch, PendingMailSend,
    ReadState,
};
use gpui_kit::base::TextSelection;
use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::*;
use gpui_kit::test::TestEventExt as _;
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    Entity, Modifiers, MouseButton, ScrollStrategy, TestAppContext, VisualTestContext, point, px,
    size,
};

use super::super::composer::bind_mail_composer_keys;
use super::{
    CompactMailPane, FollowUpFlagUpdateStatus, MailFolderNavigation, MailPermissionRequester,
    MailSyncProgress, MailView, MailViewEvent, MessageDetailState, MessageListState,
    MessageMoveMenuAction, MoveRecoveryReason, ReadStateUpdateStatus, SyncStatus,
    WellKnownFolderActionStatus, bind_message_navigation_keys, empty_message_detail_copy,
    follow_up_flag_failure_message, mail_error_message, read_state_failure_message,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

mod body_rendering;
mod cache;
mod composer;
mod follow_up_flag;
mod helpers;
mod loading_sync;
mod message_actions;
mod message_list;
mod message_move;
mod providers;
mod read_state;
mod status_copy;

use cache::*;
use helpers::*;
use providers::*;
