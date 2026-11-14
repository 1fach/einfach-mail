use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use einfach_mail_application::{
    AccountOperationError, MailCache, MailCacheError, MailOperationError, MailSendProviderError,
    MailSession, MailSyncState, MailWorkflow, MailWorkflowError, MessageBodyForDisplay,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, FolderId, FolderReference, FollowUpFlagStatus, MailAction,
    MailCapabilities, MailFolderKind, MailMessage, Mailbox, MessageBody, MessageBodyType,
    MessageDeltaLink, MessageId, ReadState,
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
use crate::test_support::mail::{
    MessageMoveActionResponse, OfflineProvider, PermissionProvider, ReadStateActionResponse,
    ReadStateProvider, SYNTHETIC_MESSAGE_LABEL_PREFIX, append_synthetic_messages, fixture,
    mail_profile_with_capabilities, message_move_provider,
};
use crate::test_support::synthetic_account_id;
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

mod body_rendering;
mod composer;
mod day_groups;
mod folder_navigation;
mod follow_up_flag;
mod helpers;
mod layout;
mod loading_sync;
mod message_actions;
mod message_list;
mod message_move;
mod message_preview;
#[cfg(feature = "ui-preview")]
mod preview_start;
mod read_state;
mod reader_command_bar;
mod status_copy;

use helpers::*;
