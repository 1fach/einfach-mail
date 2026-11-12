use std::{rc::Rc, sync::Arc};

use super::composer::{
    ComposerTarget, MailComposer, MailComposerContext, MailComposerEvent, MailPermissionRequester,
};
#[cfg(test)]
use einfach_mail_application::MailSyncProgress;
use einfach_mail_application::{
    MailCacheError, MailSession, MailSyncState, MailWorkflowError, OpenedMessage,
    WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountProfile, FolderReference, MailAction, MailFolder, MailMessage, MailReplySource,
    MessageId, PendingMailSend,
};
use gpui_kit::base::VirtualListScrollHandle;
use gpui_kit::component::StyledExt;
use gpui_kit::component::WindowExt;
use gpui_kit::component::resizable::ResizableState;
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, Pixels, Rems, ScrollStrategy, Subscription,
    Task, Window, div, prelude::*, rems, size,
};

mod follow_up_flag;
mod message_actions;
mod message_list;
mod message_move;
mod message_navigation;
mod message_reader;
mod message_write;
mod read_state;
mod synchronization;

use follow_up_flag::FollowUpFlagUpdateStatus;
#[cfg(test)]
use follow_up_flag::follow_up_flag_failure_message;
use message_move::{
    MessageMoveMenuAction, MessageMoveStatus, MoveRecoveryReason, WellKnownFolderActionStatus,
    well_known_folder_mail_action,
};
pub(crate) use message_navigation::bind_message_navigation_keys;
use message_navigation::{MESSAGE_LIST_CONTEXT, MESSAGE_READER_CONTEXT};
use read_state::ReadStateUpdateStatus;
#[cfg(test)]
use read_state::read_state_failure_message;
use synchronization::SyncStatus;

const MESSAGE_ROW_HEIGHT: Rems = rems(4.5);
const MESSAGE_LIST_DEFAULT_WIDTH: Rems = rems(22.);
const MESSAGE_LIST_MIN_WIDTH: Rems = rems(18.);
const MESSAGE_LIST_MAX_WIDTH: Rems = rems(30.);
const MESSAGE_LIST_WIDTH_STEP: Rems = rems(1.);
const MESSAGE_READER_MIN_WIDTH: Rems = rems(30.);
const MAIL_SHELL_NAVIGATION_WIDTH: Rems = rems(16.);
const MAIL_SPLIT_HANDLE_RESERVE: Rems = rems(1.);
const MAIL_COMPOSER_SHEET_WIDTH: Rems = rems(30.);
const ACCOUNT_SYNC_REQUIRED_MESSAGE: &str = "Complete a successful mail sync for this account before archiving, moving, or deleting messages.";

#[derive(Clone, Copy, Eq, PartialEq)]
enum CompactMailPane {
    MessageList,
    MessageReader,
}

pub(crate) enum MailViewEvent {
    ReviewMailWritePermission,
}

pub(crate) struct MailView {
    profile: AccountProfile,
    workflow: Arc<dyn MailSession>,
    permission_requester: Arc<dyn MailPermissionRequester>,
    pending_send: Result<Option<PendingMailSend>, MailCacheError>,
    composer: Option<Entity<MailComposer>>,
    composer_subscription: Option<Subscription>,
    folders: Vec<MailFolder>,
    selected_folder: Option<FolderReference>,
    messages: Vec<MailMessage>,
    message_list_state: MessageListState,
    compact_pane: CompactMailPane,
    pane_layout: MailPaneLayout,
    message_row_sizes: Rc<Vec<gpui_kit::Size<gpui_kit::Pixels>>>,
    message_row_rem_size: Option<Pixels>,
    message_list_focus: FocusHandle,
    message_list_scroll: VirtualListScrollHandle,
    sync_state: Option<MailSyncState>,
    sync_status: SyncStatus,
    sync_task: Option<Task<()>>,
    message_detail_state: MessageDetailState,
    message_task: Option<Task<()>>,
    read_state_update: ReadStateUpdateStatus,
    message_update_task: Option<Task<()>>,
    follow_up_flag_update: FollowUpFlagUpdateStatus,
    message_move_status: MessageMoveStatus,
    well_known_folder_action_status: WellKnownFolderActionStatus,
    move_recovery_reason: Option<MoveRecoveryReason>,
    selected_folder_write_sync_required: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct MessageListWidth(Rems);

impl MessageListWidth {
    const DEFAULT: Self = Self(MESSAGE_LIST_DEFAULT_WIDTH);

    fn with_step(self, change: MessageListWidthChange) -> Self {
        let width = match change {
            MessageListWidthChange::Narrower => self.0.0 - MESSAGE_LIST_WIDTH_STEP.0,
            MessageListWidthChange::Wider => self.0.0 + MESSAGE_LIST_WIDTH_STEP.0,
        };
        Self(rems(
            width.clamp(MESSAGE_LIST_MIN_WIDTH.0, MESSAGE_LIST_MAX_WIDTH.0),
        ))
    }

    fn from_pixels(width: Pixels, rem_size: Pixels) -> Self {
        Self(rems(
            (width / rem_size).clamp(MESSAGE_LIST_MIN_WIDTH.0, MESSAGE_LIST_MAX_WIDTH.0),
        ))
    }

    fn to_pixels(self, rem_size: Pixels) -> Pixels {
        self.0.to_pixels(rem_size)
    }

    fn is_min(self) -> bool {
        self.0 == MESSAGE_LIST_MIN_WIDTH
    }

    fn is_max(self) -> bool {
        self.0 == MESSAGE_LIST_MAX_WIDTH
    }

    fn is_disabled_for(self, change: MessageListWidthChange) -> bool {
        match change {
            MessageListWidthChange::Narrower => self.is_min(),
            MessageListWidthChange::Wider => self.is_max(),
        }
    }
}

#[derive(Clone, Copy)]
enum MessageListWidthChange {
    Narrower,
    Wider,
}

impl MessageListWidthChange {
    fn button_id(self) -> &'static str {
        match self {
            Self::Narrower => "message-list-narrower",
            Self::Wider => "message-list-wider",
        }
    }

    fn button_label(self) -> &'static str {
        match self {
            Self::Narrower => "Narrow list",
            Self::Wider => "Widen list",
        }
    }
}

struct MailPaneLayout {
    resizable_state: Entity<ResizableState>,
    message_list_width: MessageListWidth,
    rem_size: Option<Pixels>,
}

impl MailPaneLayout {
    fn new(cx: &mut Context<MailView>) -> Self {
        Self {
            resizable_state: cx.new(|_| ResizableState::default()),
            message_list_width: MessageListWidth::DEFAULT,
            rem_size: None,
        }
    }

    fn sync_rem_size(&mut self, rem_size: Pixels, cx: &mut Context<MailView>) {
        if self.rem_size.is_some_and(|previous| previous != rem_size) {
            self.resizable_state.update(cx, |state, _| state.clear());
        }
        self.rem_size = Some(rem_size);
    }

    fn resize(
        &mut self,
        change: MessageListWidthChange,
        window: &mut Window,
        cx: &mut Context<MailView>,
    ) {
        let width = self.message_list_width.with_step(change);
        if width == self.message_list_width {
            return;
        }

        self.message_list_width = width;
        let width = width.to_pixels(window.rem_size());
        self.resizable_state
            .update(cx, |state, cx| state.resize_panel(0, width, window, cx));
        cx.notify();
    }

    fn record_dragged_width(&mut self, width: Pixels, rem_size: Pixels) {
        self.message_list_width = MessageListWidth::from_pixels(width, rem_size);
    }
}

struct MailComposerPresentation {
    title: &'static str,
    context: MailComposerContext,
}

enum MessageListState {
    Ready,
    Loading,
    Failed(MailWorkflowError),
}

fn empty_message_detail_copy(
    message_list_state: &MessageListState,
    selected_folder: Option<&FolderReference>,
    messages: &[MailMessage],
) -> &'static str {
    match (message_list_state, selected_folder, messages.is_empty()) {
        (MessageListState::Ready, Some(_), false) => "Select a message to read it",
        _ => "No message selected",
    }
}

enum MessageDetailState {
    Empty,
    Loading(MessageId),
    Opened(Box<OpenedMessage>),
    MoveCompleted {
        destination_folder: FolderReference,
        destination_label: String,
        open_action_label: &'static str,
    },
    DeleteCompleted,
    Failed {
        message_id: MessageId,
        error: MailWorkflowError,
    },
}

struct MailFolderSnapshot {
    messages: Vec<MailMessage>,
    sync_state: Option<MailSyncState>,
    folder_write_sync_required: bool,
}

impl MailView {
    pub(crate) fn new(
        profile: AccountProfile,
        workflow: Arc<dyn MailSession>,
        permission_requester: Arc<dyn MailPermissionRequester>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            profile,
            workflow,
            permission_requester,
            pending_send: Err(MailCacheError::Unavailable),
            composer: None,
            composer_subscription: None,
            folders: Vec::new(),
            selected_folder: None,
            messages: Vec::new(),
            message_list_state: MessageListState::Loading,
            compact_pane: CompactMailPane::MessageList,
            pane_layout: MailPaneLayout::new(cx),
            message_row_sizes: Rc::new(Vec::new()),
            message_row_rem_size: None,
            message_list_focus: cx.focus_handle().tab_stop(true),
            message_list_scroll: VirtualListScrollHandle::new(),
            sync_state: None,
            sync_status: SyncStatus::LoadingCache,
            sync_task: None,
            message_detail_state: MessageDetailState::Empty,
            message_task: None,
            read_state_update: ReadStateUpdateStatus::Idle,
            message_update_task: None,
            follow_up_flag_update: FollowUpFlagUpdateStatus::Idle,
            message_move_status: MessageMoveStatus::Idle,
            well_known_folder_action_status: WellKnownFolderActionStatus::Idle,
            move_recovery_reason: Some(MoveRecoveryReason::InitialSyncRequired),
            selected_folder_write_sync_required: true,
        };
        view.load_cache_and_sync(cx);
        view
    }

    fn selected_message_id(&self) -> Option<&MessageId> {
        match &self.message_detail_state {
            MessageDetailState::Loading(message_id)
            | MessageDetailState::Failed { message_id, .. } => Some(message_id),
            MessageDetailState::Opened(opened) => Some(opened.message().id()),
            MessageDetailState::Empty
            | MessageDetailState::MoveCompleted { .. }
            | MessageDetailState::DeleteCompleted => None,
        }
    }

    fn set_messages(&mut self, messages: Vec<MailMessage>) {
        self.message_list_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.messages = messages;
    }

    fn update_message_row_sizes(&mut self, rem_size: Pixels) {
        if self.message_row_rem_size == Some(rem_size)
            && self.message_row_sizes.len() == self.messages.len()
        {
            return;
        }

        let row_height = MESSAGE_ROW_HEIGHT.to_pixels(rem_size);
        self.message_row_sizes = Rc::new(vec![size(Pixels::ZERO, row_height); self.messages.len()]);
        self.message_row_rem_size = Some(rem_size);
    }

    pub(crate) fn stop_pending_work(&mut self) {
        drop(self.sync_task.take());
        drop(self.message_task.take());
        drop(self.message_update_task.take());
        self.sync_status = SyncStatus::Idle;
        self.selected_folder = None;
        self.sync_state = None;
        self.folders.clear();
        self.messages.clear();
        self.message_list_state = MessageListState::Ready;
        self.message_detail_state = MessageDetailState::Empty;
        self.read_state_update = ReadStateUpdateStatus::Idle;
        self.follow_up_flag_update = FollowUpFlagUpdateStatus::Idle;
        self.message_move_status = MessageMoveStatus::Idle;
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        self.move_recovery_reason = None;
        self.selected_folder_write_sync_required = true;
    }

    pub(crate) fn can_navigate_away(&self) -> bool {
        !self.message_update_in_progress()
    }

    fn message_update_in_progress(&self) -> bool {
        self.message_update_task.is_some()
    }

    fn can_update_message_in(&self, folder: &FolderReference, action: MailAction) -> bool {
        self.profile.capabilities().mail().supports(action)
            && !self.sync_status.is_sync_active()
            && !self.message_update_in_progress()
            && !matches!(self.sync_status, SyncStatus::Failed(_))
            && self.selected_folder.as_ref() == Some(folder)
            && !self.selected_folder_write_sync_required
    }

    fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .profile
            .capabilities()
            .mail()
            .supports(MailAction::Send)
            || self.sync_status.is_sync_active()
        {
            return;
        }
        let title = if matches!(&self.pending_send, Ok(Some(_))) {
            "Resolve pending send"
        } else {
            "New message"
        };
        let presentation = MailComposerPresentation {
            title,
            context: self.composer_context(ComposerTarget::NewMessage),
        };
        self.present_composer(presentation, window, cx);
    }

    fn open_reply_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_open_reply(window, cx) {
            return;
        }
        let Some(target) = self.selected_reply_target() else {
            return;
        };
        let presentation = MailComposerPresentation {
            title: "Reply",
            context: self.composer_context(target),
        };
        self.present_composer(presentation, window, cx);
    }

    fn can_open_reply(&self, window: &mut Window, cx: &mut App) -> bool {
        self.profile
            .capabilities()
            .mail()
            .supports(MailAction::Reply)
            && !self.sync_status.is_sync_active()
            && !self.message_update_in_progress()
            && matches!(&self.pending_send, Ok(None))
            && !window.has_active_sheet(cx)
    }

    fn selected_reply_target(&self) -> Option<ComposerTarget> {
        let MessageDetailState::Opened(opened) = &self.message_detail_state else {
            return None;
        };
        let message = opened.message();
        if self.selected_folder.as_ref() != Some(message.folder()) {
            return None;
        }
        Some(ComposerTarget::Reply {
            source: MailReplySource::new(message.folder().clone(), message.id().clone()),
            subject: message.metadata().subject().map(str::to_owned),
        })
    }

    fn composer_context(&self, target: ComposerTarget) -> MailComposerContext {
        MailComposerContext {
            workflow: Arc::clone(&self.workflow),
            permission_requester: Arc::clone(&self.permission_requester),
            account_id: self.profile.id().clone(),
            pending: self.pending_send.clone(),
            target,
        }
    }

    fn present_composer(
        &mut self,
        presentation: MailComposerPresentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let MailComposerPresentation { title, context } = presentation;
        let composer = cx.new(|cx| MailComposer::new(context, window, cx));
        let subscription = self.subscribe_to_composer(&composer, cx);
        self.composer = Some(composer.clone());
        self.composer_subscription = Some(subscription);
        open_composer_sheet(window, cx, title, composer);
    }

    fn subscribe_to_composer(
        &mut self,
        composer: &Entity<MailComposer>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe(
            composer,
            |view, _, event: &MailComposerEvent, cx| match event {
                MailComposerEvent::PendingSendChanged(pending) => {
                    view.pending_send = Ok(pending.clone());
                    cx.notify();
                }
            },
        )
    }
}

fn open_composer_sheet(
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    composer: Entity<MailComposer>,
) {
    window.open_sheet(cx, move |sheet, _, cx| {
        let composer_for_close = composer.clone();
        sheet
            .overlay_closable(!composer.read(cx).operation_in_progress())
            .title(div().text_base().font_bold().child(title))
            .size(MAIL_COMPOSER_SHEET_WIDTH)
            .on_close(move |_, window, cx| {
                if composer_for_close.read(cx).operation_in_progress() {
                    open_composer_sheet(window, cx, title, composer_for_close.clone());
                }
            })
            .child(composer.clone())
    });
}

impl EventEmitter<MailViewEvent> for MailView {}

async fn load_folder_snapshot(
    workflow: &dyn MailSession,
    folder: &FolderReference,
) -> Result<MailFolderSnapshot, MailWorkflowError> {
    Ok(MailFolderSnapshot {
        messages: workflow.list_cached_messages(folder).await?,
        sync_state: workflow.cached_sync_state(folder).await?,
        folder_write_sync_required: workflow.folder_write_sync_required(folder).await?,
    })
}

fn message_sender(message: &MailMessage) -> String {
    message
        .metadata()
        .origin()
        .from_mailbox()
        .or_else(|| message.metadata().origin().sender())
        .map_or_else(
            || "Unknown sender".to_owned(),
            |mailbox| match mailbox.display_name().unwrap_or_default().trim() {
                "" => mailbox.address().to_owned(),
                name => format!("{name} <{}>", mailbox.address()),
            },
        )
}

fn mail_error_message(error: MailWorkflowError) -> &'static str {
    use einfach_mail_application::{MailCacheError, MailOperationError};

    match error {
        MailWorkflowError::Provider(MailOperationError::AccessDenied) => {
            "Microsoft denied mail access. Review account permissions and try again."
        }
        MailWorkflowError::Provider(MailOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and connect again."
        }
        MailWorkflowError::Provider(MailOperationError::MoveOutcomeUnknown) => {
            "The message may have moved. Sync mail before trying again."
        }
        MailWorkflowError::Cache(MailCacheError::MissingEncryptionKey) => {
            "The encrypted mail cache key is missing. Remove the local cache and sync again."
        }
        MailWorkflowError::Cache(MailCacheError::SyncStateChanged) => {
            "Another sync updated this folder. Choose Sync to continue."
        }
        MailWorkflowError::Cache(MailCacheError::Unavailable) => {
            "The encrypted mail cache is unavailable. Unlock your Linux keyring and try again."
        }
        MailWorkflowError::Cache(MailCacheError::MessageNotCached) => {
            "This message is no longer in the local cache. Sync the folder and try again."
        }
        MailWorkflowError::AccountMismatch => {
            "This folder belongs to another account. Open the matching account and try again."
        }
        MailWorkflowError::OperationInProgress => {
            "Another mail operation is active. Try again shortly."
        }
        MailWorkflowError::InvalidMoveDestination => {
            "The destination folder is unavailable. Sync mail and try again."
        }
        MailWorkflowError::AccountSyncRequired => ACCOUNT_SYNC_REQUIRED_MESSAGE,
        MailWorkflowError::AlreadyInWellKnownFolder(WellKnownMailFolder::Archive) => {
            "This message is already in Archive."
        }
        MailWorkflowError::AlreadyInWellKnownFolder(WellKnownMailFolder::DeletedItems) => {
            "This message is already in Deleted Items. Move it to another folder instead."
        }
        MailWorkflowError::WellKnownFolderUnavailable(WellKnownMailFolder::Archive) => {
            "Archive is not available in the visible folder list. Sync mail and try again."
        }
        MailWorkflowError::WellKnownFolderUnavailable(WellKnownMailFolder::DeletedItems) => {
            "Deleted Items is not available in the visible folder list. Sync mail and try again."
        }
        MailWorkflowError::FolderSyncRequired => {
            "Sync this folder before archiving, moving, or deleting messages."
        }
        MailWorkflowError::RepeatedCursor => {
            "Sync stopped because the service repeated a page. Try syncing again."
        }
        MailWorkflowError::RepeatedDeltaReset => {
            "The mail service reset the rebuild again. Choose Sync to resume from its saved checkpoint."
        }
        MailWorkflowError::PageLimitExceeded => {
            "Sync paused after many pages. Choose Sync to continue from its saved checkpoint."
        }
        MailWorkflowError::ResponseLimitExceeded => {
            "Sync paused after receiving a large amount of mail data. Choose Sync to continue from its saved checkpoint."
        }
        _ => "Couldn't complete this mail request. Check your connection and try again.",
    }
}

mod folder_navigation;
mod render;

pub(crate) use folder_navigation::MailFolderNavigation;

#[cfg(test)]
mod tests;
