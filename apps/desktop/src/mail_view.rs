use std::{rc::Rc, sync::Arc, time::SystemTime};

use crate::list_navigation::ListNavigation;
use crate::mail_composer::{
    ComposerTarget, MailComposer, MailComposerContext, MailComposerEvent, MailPermissionRequester,
};
use crate::sync_age::relative_age_label;
use einfach_mail_application::{
    FollowUpFlagUpdateOutcome, MailCacheError, MailOperationError, MailSession, MailSyncProgress,
    MailSyncReport, MailSyncState, MailWorkflowError, MessageMoveOutcome, OpenedMessage,
    ReadStateUpdateOutcome, WellKnownFolderMoveOutcome, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, FolderReference, FollowUpFlagStatus, MailFolder, MailMessage,
    MailReplySource, MessageId, MessageMetadataPatch, PendingMailSend, ReadState,
};
use gpui_kit::base::VirtualListScrollHandle;
use gpui_kit::component::StyledExt;
use gpui_kit::component::WindowExt;
use gpui_kit::{
    App, AsyncApp, Context, Entity, EventEmitter, FocusHandle, KeyBinding, Pixels, Role,
    ScrollStrategy, Subscription, Task, Window, div, prelude::*, px, size,
};

gpui_kit::actions!(
    mail_message_list,
    [
        SelectPreviousMessage,
        SelectNextMessage,
        SelectFirstMessage,
        SelectLastMessage,
    ]
);

const MESSAGE_LIST_CONTEXT: &str = "MailMessageList";
const MESSAGE_ROW_HEIGHT: Pixels = px(72.);
const ACCOUNT_SYNC_REQUIRED_MESSAGE: &str = "Complete a successful mail sync for this account before archiving, moving, or deleting messages.";

pub(crate) fn bind_message_navigation_keys(cx: &mut App) {
    let context = Some(MESSAGE_LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPreviousMessage, context),
        KeyBinding::new("down", SelectNextMessage, context),
        KeyBinding::new("home", SelectFirstMessage, context),
        KeyBinding::new("end", SelectLastMessage, context),
    ]);
}

pub(crate) enum MailViewEvent {
    BackToAccount,
    ReviewMailWritePermission,
}

pub(crate) struct MailView {
    profile: AccountProfile,
    workflow: Arc<dyn MailSession>,
    permission_requester: Option<Arc<dyn MailPermissionRequester>>,
    pending_send: Option<PendingMailSend>,
    pending_send_error: Option<MailCacheError>,
    composer: Option<Entity<MailComposer>>,
    composer_subscription: Option<Subscription>,
    folders: Vec<MailFolder>,
    selected_folder: Option<FolderReference>,
    messages: Vec<MailMessage>,
    message_list_state: MessageListState,
    message_row_sizes: Rc<Vec<gpui_kit::Size<gpui_kit::Pixels>>>,
    message_list_focus: FocusHandle,
    message_list_scroll: VirtualListScrollHandle,
    sync_state: Option<MailSyncState>,
    sync_status: SyncStatus,
    sync_running: bool,
    sync_task: Option<Task<()>>,
    selected_message_id: Option<MessageId>,
    message_detail_state: MessageDetailState,
    message_task: Option<Task<()>>,
    read_state_update: ReadStateUpdateStatus,
    read_state_task: Option<Task<()>>,
    follow_up_flag_update: FollowUpFlagUpdateStatus,
    follow_up_flag_task: Option<Task<()>>,
    message_move_status: MessageMoveStatus,
    message_move_task: Option<Task<()>>,
    well_known_folder_action_status: WellKnownFolderActionStatus,
    well_known_folder_action_task: Option<Task<()>>,
    move_recovery_reason: Option<MoveRecoveryReason>,
    selected_folder_write_sync_required: bool,
}

struct MailComposerPresentation {
    title: &'static str,
    context: MailComposerContext,
}

enum SyncStatus {
    LoadingCache,
    Syncing,
    Rebuilding,
    UpToDate,
    Failed(MailWorkflowError),
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
    Loading,
    Opened(Box<OpenedMessage>),
    MoveCompleted {
        destination_folder: FolderReference,
        destination_label: String,
        open_action_label: &'static str,
    },
    DeleteCompleted,
    Failed(MailWorkflowError),
}

enum MessageMoveStatus {
    Idle,
    Pending {
        message_id: MessageId,
        destination_label: String,
    },
    Failed {
        message_id: MessageId,
        error: MailWorkflowError,
    },
}

enum WellKnownFolderActionStatus {
    Idle,
    Pending {
        message_id: MessageId,
        destination: WellKnownMailFolder,
    },
    Failed {
        message_id: MessageId,
        destination: WellKnownMailFolder,
        error: MailWorkflowError,
    },
}

#[derive(Clone, Copy)]
enum MoveRecoveryReason {
    InitialSyncRequired,
    OutcomeUnknown,
    CacheNeedsSync,
}

#[derive(Clone)]
struct MessageMoveRequest {
    source_folder: FolderReference,
    message_id: MessageId,
    destination_folder: FolderReference,
    destination_label: String,
}

#[derive(Clone)]
struct WellKnownFolderActionRequest {
    source_folder: FolderReference,
    message_id: MessageId,
    destination: WellKnownMailFolder,
}

#[derive(Clone)]
struct MessageMoveMenuAction {
    selected_message_id: MessageId,
    destination_folder: FolderReference,
    destination_label: String,
}

enum ReadStateUpdateStatus {
    Idle,
    Pending {
        message_id: MessageId,
        requested: ReadState,
    },
    Confirmed {
        message_id: MessageId,
        state: ReadState,
    },
    ConfirmedNeedsSync {
        message_id: MessageId,
        state: ReadState,
    },
    Failed {
        message_id: MessageId,
        requested: ReadState,
        error: MailWorkflowError,
    },
}

enum ReadStateFeedback {
    Status(String),
    Failure {
        message: String,
        error: MailWorkflowError,
    },
}

impl ReadStateFeedback {
    fn into_parts(self) -> (String, Role, bool) {
        match self {
            Self::Status(message) => (message, Role::Status, false),
            Self::Failure { message, error } => (
                message,
                Role::Alert,
                matches!(
                    error,
                    MailWorkflowError::Provider(MailOperationError::AccessDenied)
                ),
            ),
        }
    }
}

impl ReadStateUpdateStatus {
    fn is_pending_for(&self, message_id: &MessageId) -> bool {
        matches!(
            self,
            Self::Pending {
                message_id: pending_id,
                ..
            } if pending_id == message_id
        )
    }

    fn feedback_for(&self, message_id: &MessageId) -> Option<ReadStateFeedback> {
        if self.message_id()? != message_id {
            return None;
        }
        self.feedback()
    }

    fn message_id(&self) -> Option<&MessageId> {
        match self {
            Self::Idle => None,
            Self::Pending { message_id, .. }
            | Self::Confirmed { message_id, .. }
            | Self::ConfirmedNeedsSync { message_id, .. }
            | Self::Failed { message_id, .. } => Some(message_id),
        }
    }

    fn feedback(&self) -> Option<ReadStateFeedback> {
        match self {
            Self::Idle => None,
            Self::Pending { requested, .. } => Some(ReadStateFeedback::Status(
                read_state_progress_message(*requested),
            )),
            Self::Confirmed { state, .. } => Some(ReadStateFeedback::Status(
                read_state_confirmed_message(*state).to_owned(),
            )),
            Self::ConfirmedNeedsSync { state, .. } => Some(ReadStateFeedback::Status(
                read_state_cache_sync_message(*state).to_owned(),
            )),
            Self::Failed {
                requested, error, ..
            } => Some(ReadStateFeedback::Failure {
                message: read_state_failure_message(*requested, *error),
                error: *error,
            }),
        }
    }
}

struct ReadStateUpdateRequest {
    folder: FolderReference,
    message_id: MessageId,
    requested: ReadState,
}

enum FollowUpFlagUpdateStatus {
    Idle,
    Pending {
        message_id: MessageId,
        requested: FollowUpFlagStatus,
    },
    Confirmed {
        message_id: MessageId,
        status: FollowUpFlagStatus,
    },
    ConfirmedNeedsSync {
        message_id: MessageId,
        status: FollowUpFlagStatus,
    },
    Failed {
        message_id: MessageId,
        requested: FollowUpFlagStatus,
        error: MailWorkflowError,
    },
}

struct FollowUpFlagUpdateRequest {
    folder: FolderReference,
    message_id: MessageId,
    requested: FollowUpFlagStatus,
}

impl FollowUpFlagUpdateStatus {
    fn is_pending_for(&self, message_id: &MessageId) -> bool {
        matches!(
            self,
            Self::Pending {
                message_id: pending_id,
                ..
            } if pending_id == message_id
        )
    }

    fn action_label(
        &self,
        message_id: &MessageId,
        flag_status: Option<FollowUpFlagStatus>,
    ) -> &'static str {
        if self.is_pending_for(message_id) {
            return "Updating…";
        }
        match flag_status {
            Some(FollowUpFlagStatus::Flagged) => "Clear flag",
            Some(FollowUpFlagStatus::Complete) => "Reopen flag",
            Some(FollowUpFlagStatus::NotFlagged) | None => "Flag",
        }
    }
}

struct MailSnapshot {
    folders: Vec<MailFolder>,
    selected_folder: Option<FolderReference>,
    messages: Vec<MailMessage>,
    sync_state: Option<MailSyncState>,
    folder_write_sync_required: bool,
    pending_send: Result<Option<PendingMailSend>, MailCacheError>,
}

impl MailView {
    pub(crate) fn new(
        profile: AccountProfile,
        workflow: Arc<dyn MailSession>,
        permission_requester: Option<Arc<dyn MailPermissionRequester>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            profile,
            workflow,
            permission_requester,
            pending_send: None,
            pending_send_error: Some(MailCacheError::Unavailable),
            composer: None,
            composer_subscription: None,
            folders: Vec::new(),
            selected_folder: None,
            messages: Vec::new(),
            message_list_state: MessageListState::Loading,
            message_row_sizes: Rc::new(Vec::new()),
            message_list_focus: cx.focus_handle().tab_stop(true),
            message_list_scroll: VirtualListScrollHandle::new(),
            sync_state: None,
            sync_status: SyncStatus::LoadingCache,
            sync_running: true,
            sync_task: None,
            selected_message_id: None,
            message_detail_state: MessageDetailState::Empty,
            message_task: None,
            read_state_update: ReadStateUpdateStatus::Idle,
            read_state_task: None,
            follow_up_flag_update: FollowUpFlagUpdateStatus::Idle,
            follow_up_flag_task: None,
            message_move_status: MessageMoveStatus::Idle,
            message_move_task: None,
            well_known_folder_action_status: WellKnownFolderActionStatus::Idle,
            well_known_folder_action_task: None,
            move_recovery_reason: Some(MoveRecoveryReason::InitialSyncRequired),
            selected_folder_write_sync_required: true,
        };
        view.load_cache_and_sync(cx);
        view
    }

    fn load_cache_and_sync(&mut self, cx: &mut Context<Self>) {
        let workflow = Arc::clone(&self.workflow);
        let account_id = self.profile.id().clone();
        self.sync_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let initial_snapshot = load_snapshot(workflow.as_ref(), &account_id, None).await;
            let has_snapshot = initial_snapshot.is_ok();
            let _ = this.update(cx, |view, cx| match initial_snapshot {
                Ok(snapshot) => {
                    view.apply_snapshot(snapshot);
                    view.sync_status = SyncStatus::Syncing;
                    cx.notify();
                }
                Err(error) => {
                    view.sync_running = false;
                    view.sync_status = SyncStatus::Failed(error);
                    view.message_list_state = MessageListState::Failed(error);
                    cx.notify();
                }
            });
            if !has_snapshot {
                return;
            }

            let sync_result = synchronize_with_progress(
                workflow.as_ref(),
                &account_id,
                SystemTime::now(),
                |progress| {
                    let _ = this.update(cx, |view, cx| {
                        view.apply_sync_progress(progress);
                        cx.notify();
                    });
                },
            )
            .await;
            let Ok(selected_folder) = this.read_with(cx, |view, _| view.selected_folder.clone())
            else {
                return;
            };
            let snapshot =
                load_snapshot(workflow.as_ref(), &account_id, selected_folder.as_ref()).await;
            let _ = this.update(cx, |view, cx| {
                view.sync_running = false;
                view.sync_task = None;
                view.finish_sync(sync_result.map(|_| ()), snapshot);
                cx.notify();
            });
        }));
    }

    fn apply_snapshot(&mut self, snapshot: MailSnapshot) {
        self.folders = snapshot.folders;
        self.selected_folder = snapshot.selected_folder;
        self.set_messages(snapshot.messages);
        self.sync_state = snapshot.sync_state;
        self.selected_folder_write_sync_required = snapshot.folder_write_sync_required;
        match snapshot.pending_send {
            Ok(pending) => {
                self.pending_send = pending;
                self.pending_send_error = None;
            }
            Err(error) => {
                self.pending_send = None;
                self.pending_send_error = Some(error);
            }
        }
        self.selected_message_id = None;
        self.message_list_state = MessageListState::Ready;
        self.message_detail_state = MessageDetailState::Empty;
        self.message_task = None;
        self.read_state_update = ReadStateUpdateStatus::Idle;
        self.follow_up_flag_update = FollowUpFlagUpdateStatus::Idle;
        self.message_move_status = MessageMoveStatus::Idle;
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
    }

    fn set_messages(&mut self, messages: Vec<MailMessage>) {
        self.message_list_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.message_row_sizes = Rc::new(vec![size(px(1.), MESSAGE_ROW_HEIGHT); messages.len()]);
        self.messages = messages;
    }

    fn apply_sync_progress(&mut self, progress: MailSyncProgress) {
        match progress {
            MailSyncProgress::Rebuilding => self.sync_status = SyncStatus::Rebuilding,
        }
    }

    fn finish_sync(
        &mut self,
        sync_result: Result<(), MailWorkflowError>,
        snapshot: Result<MailSnapshot, MailWorkflowError>,
    ) {
        let sync_reconciled = sync_result.is_ok() && snapshot.is_ok();
        let (status, snapshot) = match (sync_result, snapshot) {
            (_, Err(error)) => (SyncStatus::Failed(error), None),
            (Err(error), Ok(snapshot)) => (SyncStatus::Failed(error), Some(snapshot)),
            (Ok(_), Ok(snapshot)) => (SyncStatus::UpToDate, Some(snapshot)),
        };
        if let Some(snapshot) = snapshot {
            self.apply_snapshot(snapshot);
        }
        if sync_reconciled {
            self.move_recovery_reason = None;
            self.message_move_status = MessageMoveStatus::Idle;
            self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        }
        self.sync_status = status;
    }

    pub(crate) fn stop_pending_work(&mut self) {
        drop(self.sync_task.take());
        drop(self.message_task.take());
        drop(self.read_state_task.take());
        drop(self.follow_up_flag_task.take());
        drop(self.message_move_task.take());
        drop(self.well_known_folder_action_task.take());
        self.sync_running = false;
        self.selected_folder = None;
        self.selected_message_id = None;
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

    fn select_folder(&mut self, folder: FolderReference, cx: &mut Context<Self>) {
        if self.sync_running || self.message_update_in_progress() {
            return;
        }
        if self.selected_folder.as_ref() == Some(&folder) {
            return;
        }
        self.message_move_status = MessageMoveStatus::Idle;
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        self.selected_folder = Some(folder.clone());
        self.selected_folder_write_sync_required = true;
        self.sync_state = None;
        self.selected_message_id = None;
        self.message_list_state = MessageListState::Loading;
        self.message_detail_state = MessageDetailState::Empty;
        self.read_state_update = ReadStateUpdateStatus::Idle;
        self.follow_up_flag_update = FollowUpFlagUpdateStatus::Idle;
        self.set_messages(Vec::new());
        let workflow = Arc::clone(&self.workflow);
        self.message_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let messages = workflow.list_cached_messages(&folder).await;
            let sync_state = workflow.cached_sync_state(&folder).await;
            let write_sync_required = workflow.folder_write_sync_required(&folder).await;
            let _ = this.update(cx, |view, cx| {
                if view.selected_folder.as_ref() != Some(&folder) {
                    return;
                }
                match (messages, sync_state, write_sync_required) {
                    (Ok(messages), Ok(sync_state), Ok(write_sync_required)) => {
                        view.set_messages(messages);
                        view.sync_state = sync_state;
                        view.selected_folder_write_sync_required = write_sync_required;
                        view.message_list_state = MessageListState::Ready;
                        view.message_detail_state = MessageDetailState::Empty;
                    }
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                        view.message_list_state = MessageListState::Failed(error);
                    }
                }
                view.message_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn select_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        if self.message_update_in_progress() {
            return;
        }
        if !self
            .messages
            .iter()
            .any(|message| message.id() == &message_id)
        {
            return;
        }
        let Some(folder) = self.selected_folder.clone() else {
            return;
        };
        self.selected_message_id = Some(message_id.clone());
        self.message_detail_state = MessageDetailState::Loading;
        let workflow = Arc::clone(&self.workflow);
        let request_message_id = message_id.clone();
        self.message_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            // Keep HTML sanitization off GPUI's event-loop executor.
            let opened = cx
                .background_executor()
                .spawn(async move { workflow.open_message(&folder, &request_message_id).await })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.selected_message_id.as_ref() != Some(&message_id) {
                    return;
                }
                view.message_detail_state = match opened {
                    Ok(message) => MessageDetailState::Opened(Box::new(message)),
                    Err(error) => MessageDetailState::Failed(error),
                };
                view.message_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn navigate_message(&mut self, navigation: ListNavigation, cx: &mut Context<Self>) {
        if self.sync_running || self.message_update_in_progress() {
            return;
        }
        let selected_index = self.selected_message_id.as_ref().and_then(|selected| {
            self.messages
                .iter()
                .position(|message| message.id() == selected)
        });
        let Some(target_index) = navigation.target_index(selected_index, self.messages.len())
        else {
            return;
        };
        let Some(message) = self.messages.get(target_index) else {
            return;
        };
        if self.selected_message_id.as_ref() == Some(message.id()) {
            return;
        }
        self.message_list_scroll
            .scroll_to_item(target_index, ScrollStrategy::Nearest);
        self.select_message(message.id().clone(), cx);
    }

    fn select_message_from_option(
        &mut self,
        message_id: MessageId,
        expected_folder: &FolderReference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_folder.as_ref() != Some(expected_folder) {
            return;
        }
        self.message_list_focus.focus(window, cx);
        self.select_message(message_id, cx);
    }

    fn on_select_previous_message(
        &mut self,
        _: &SelectPreviousMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(ListNavigation::Previous, cx);
    }

    fn on_select_next_message(
        &mut self,
        _: &SelectNextMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(ListNavigation::Next, cx);
    }

    fn on_select_first_message(
        &mut self,
        _: &SelectFirstMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(ListNavigation::First, cx);
    }

    fn on_select_last_message(
        &mut self,
        _: &SelectLastMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(ListNavigation::Last, cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.sync_running || self.message_update_in_progress() {
            return;
        }
        self.sync_running = true;
        self.sync_status = SyncStatus::Syncing;
        self.selected_message_id = None;
        self.message_detail_state = MessageDetailState::Empty;
        let workflow = Arc::clone(&self.workflow);
        let account_id = self.profile.id().clone();
        let selected_folder = self.selected_folder.clone();
        self.sync_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let sync_result = synchronize_with_progress(
                workflow.as_ref(),
                &account_id,
                SystemTime::now(),
                |progress| {
                    let _ = this.update(cx, |view, cx| {
                        view.apply_sync_progress(progress);
                        cx.notify();
                    });
                },
            )
            .await;
            let snapshot =
                load_snapshot(workflow.as_ref(), &account_id, selected_folder.as_ref()).await;
            let _ = this.update(cx, |view, cx| {
                view.sync_running = false;
                view.sync_task = None;
                view.finish_sync(sync_result.map(|_| ()), snapshot);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn update_selected_read_state(&mut self, cx: &mut Context<Self>) {
        if self.sync_running || self.message_update_in_progress() {
            return;
        }
        let Some(request) = self.selected_read_state_request() else {
            return;
        };
        let workflow = Arc::clone(&self.workflow);
        self.read_state_update = ReadStateUpdateStatus::Pending {
            message_id: request.message_id.clone(),
            requested: request.requested,
        };
        self.read_state_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let update_outcome = workflow
                .set_message_read_state(&request.folder, &request.message_id, request.requested)
                .await;
            let write_sync_required = workflow
                .folder_write_sync_required(&request.folder)
                .await
                .unwrap_or(true);
            let _ = this.update(cx, |view, cx| {
                view.finish_read_state_update(request, update_outcome, write_sync_required);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn selected_read_state_request(&self) -> Option<ReadStateUpdateRequest> {
        let folder = self.selected_folder.as_ref()?;
        if !self.can_update_message_in(folder) {
            return None;
        }
        let folder = folder.clone();
        let message_id = self.selected_message_id.clone()?;
        let current_state = self
            .messages
            .iter()
            .find(|message| message.id() == &message_id)?
            .metadata()
            .read_state();
        let requested = match current_state {
            ReadState::Read => ReadState::Unread,
            ReadState::Unread => ReadState::Read,
        };
        Some(ReadStateUpdateRequest {
            folder,
            message_id,
            requested,
        })
    }

    fn finish_read_state_update(
        &mut self,
        request: ReadStateUpdateRequest,
        update_outcome: Result<ReadStateUpdateOutcome, MailWorkflowError>,
        write_sync_required: bool,
    ) {
        self.read_state_update = if self.selected_folder.as_ref() == Some(&request.folder) {
            self.selected_folder_write_sync_required = write_sync_required;
            self.apply_read_state_result(request, update_outcome)
        } else {
            ReadStateUpdateStatus::Idle
        };
        self.read_state_task = None;
    }

    fn apply_read_state_result(
        &mut self,
        request: ReadStateUpdateRequest,
        update_outcome: Result<ReadStateUpdateOutcome, MailWorkflowError>,
    ) -> ReadStateUpdateStatus {
        match update_outcome {
            Ok(ReadStateUpdateOutcome::ConfirmedAndCached(state)) => {
                self.apply_read_state_to_message(&request.message_id, state);
                ReadStateUpdateStatus::Confirmed {
                    message_id: request.message_id,
                    state,
                }
            }
            Ok(ReadStateUpdateOutcome::ConfirmedNeedsSync { state, .. }) => {
                self.apply_read_state_to_message(&request.message_id, state);
                ReadStateUpdateStatus::ConfirmedNeedsSync {
                    message_id: request.message_id,
                    state,
                }
            }
            Err(error) => ReadStateUpdateStatus::Failed {
                message_id: request.message_id,
                requested: request.requested,
                error,
            },
        }
    }

    fn apply_read_state_to_message(&mut self, message_id: &MessageId, state: ReadState) {
        let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id() == message_id)
        else {
            return;
        };
        let mut patch = MessageMetadataPatch::new();
        patch.set_read_state(state);
        message.apply_patch(patch);
    }

    fn update_selected_follow_up_flag(&mut self, cx: &mut Context<Self>) {
        if self.sync_running || self.message_update_in_progress() {
            return;
        }
        let Some(request) = self.selected_follow_up_flag_request() else {
            return;
        };
        let workflow = Arc::clone(&self.workflow);
        self.follow_up_flag_update = FollowUpFlagUpdateStatus::Pending {
            message_id: request.message_id.clone(),
            requested: request.requested,
        };
        self.follow_up_flag_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let flag_update_outcome = workflow
                .set_message_follow_up_flag_status(
                    &request.folder,
                    &request.message_id,
                    request.requested,
                )
                .await;
            let write_sync_required = workflow
                .folder_write_sync_required(&request.folder)
                .await
                .unwrap_or(true);
            let _ = this.update(cx, |view, cx| {
                view.finish_follow_up_flag_update(
                    request,
                    flag_update_outcome,
                    write_sync_required,
                );
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn move_selected_message_to(
        &mut self,
        destination_folder: FolderReference,
        destination_label: String,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.prepare_message_move(destination_folder, destination_label) else {
            return;
        };
        self.start_message_move(request, cx);
    }

    fn apply_message_move_menu_action(
        &mut self,
        action: MessageMoveMenuAction,
        cx: &mut Context<Self>,
    ) {
        if self.selected_message_id.as_ref() != Some(&action.selected_message_id) {
            return;
        }
        self.move_selected_message_to(action.destination_folder, action.destination_label, cx);
    }

    fn prepare_message_move(
        &self,
        destination_folder: FolderReference,
        destination_label: String,
    ) -> Option<MessageMoveRequest> {
        if !self.message_move_allowed() {
            return None;
        }
        let source_folder = self.selected_folder.clone()?;
        let message_id = self.selected_message_id.clone()?;
        if source_folder == destination_folder
            || source_folder.account_id() != destination_folder.account_id()
        {
            return None;
        }
        Some(MessageMoveRequest {
            source_folder,
            message_id,
            destination_folder,
            destination_label,
        })
    }

    fn start_well_known_folder_action(
        &mut self,
        destination: WellKnownMailFolder,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.prepare_well_known_folder_action(destination) else {
            return;
        };
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Pending {
            message_id: request.message_id.clone(),
            destination,
        };
        self.spawn_well_known_folder_action(request, cx);
    }

    fn spawn_well_known_folder_action(
        &mut self,
        request: WellKnownFolderActionRequest,
        cx: &mut Context<Self>,
    ) {
        let workflow = Arc::clone(&self.workflow);
        self.well_known_folder_action_task =
            Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
                let action_result = workflow
                    .move_message_to_well_known_folder(
                        &request.source_folder,
                        &request.message_id,
                        request.destination,
                    )
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.finish_well_known_folder_action(request, action_result);
                    cx.notify();
                });
            }));
        cx.notify();
    }

    fn prepare_well_known_folder_action(
        &self,
        destination: WellKnownMailFolder,
    ) -> Option<WellKnownFolderActionRequest> {
        if !self.message_move_allowed() {
            return None;
        }
        Some(WellKnownFolderActionRequest {
            source_folder: self.selected_folder.clone()?,
            message_id: self.selected_message_id.clone()?,
            destination,
        })
    }

    fn message_move_allowed(&self) -> bool {
        matches!(&self.sync_status, SyncStatus::UpToDate)
            && !self.sync_running
            && !self.selected_folder_write_sync_required
            && !self.message_update_in_progress()
            && self.move_recovery_reason.is_none()
    }

    fn sync_status_label(&self) -> String {
        match &self.sync_status {
            SyncStatus::LoadingCache => "Opening encrypted cache…".to_owned(),
            SyncStatus::Syncing => {
                let label = if self
                    .sync_state
                    .as_ref()
                    .is_some_and(MailSyncState::is_rebuilding)
                {
                    "Rebuilding mail"
                } else {
                    "Syncing"
                };
                self.sync_age_label(label)
            }
            SyncStatus::Rebuilding => self.sync_age_label("Rebuilding mail"),
            SyncStatus::UpToDate => self.sync_age_label("Up to date"),
            SyncStatus::Failed(error) => {
                let rebuild_resume = self
                    .sync_state
                    .as_ref()
                    .is_some_and(MailSyncState::is_rebuilding)
                    && *error != MailWorkflowError::RepeatedDeltaReset;
                let cached_age = self.sync_state.as_ref().and_then(|state| {
                    state.last_synchronized_at().map(|last_sync| {
                        format!(" Cached mail from {}.", relative_age_label(last_sync))
                    })
                });
                format!(
                    "{}{}{}",
                    mail_error_message(*error),
                    cached_age.unwrap_or_default(),
                    if rebuild_resume {
                        " The saved mail rebuild is incomplete. Choose Sync to resume it."
                    } else {
                        ""
                    }
                )
            }
        }
    }

    fn sync_age_label(&self, prefix: &str) -> String {
        self.sync_state
            .as_ref()
            .and_then(MailSyncState::last_synchronized_at)
            .map_or_else(
                || prefix.to_owned(),
                |time| format!("{prefix} · {}", relative_age_label(time)),
            )
    }

    fn message_update_in_progress(&self) -> bool {
        self.read_state_task.is_some()
            || self.follow_up_flag_task.is_some()
            || self.message_move_task.is_some()
            || self.well_known_folder_action_task.is_some()
    }

    fn can_update_message_in(&self, folder: &FolderReference) -> bool {
        !self.sync_running
            && !self.message_update_in_progress()
            && !matches!(self.sync_status, SyncStatus::Failed(_))
            && self.selected_folder.as_ref() == Some(folder)
            && !self.selected_folder_write_sync_required
    }

    fn start_message_move(&mut self, request: MessageMoveRequest, cx: &mut Context<Self>) {
        let workflow = Arc::clone(&self.workflow);
        self.message_move_status = MessageMoveStatus::Pending {
            message_id: request.message_id.clone(),
            destination_label: request.destination_label.clone(),
        };
        let operation_request = request.clone();
        self.message_move_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let move_result = workflow
                .move_message(
                    &operation_request.source_folder,
                    &operation_request.message_id,
                    &operation_request.destination_folder,
                )
                .await;
            let _ = this.update(cx, |view, cx| {
                view.finish_message_move(request, move_result);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn finish_message_move(
        &mut self,
        request: MessageMoveRequest,
        move_result: Result<MessageMoveOutcome, MailWorkflowError>,
    ) {
        self.message_move_task = None;
        match move_result {
            Ok(outcome) => self.finish_confirmed_message_move(request, outcome),
            Err(error) => self.finish_failed_message_move(request.message_id, error),
        }
    }

    fn finish_confirmed_message_move(
        &mut self,
        request: MessageMoveRequest,
        outcome: MessageMoveOutcome,
    ) {
        match outcome {
            MessageMoveOutcome::ConfirmedAndCached => {
                self.remove_confirmed_message(&request.message_id);
                self.message_move_status = MessageMoveStatus::Idle;
                self.show_message_move_completion(
                    request.destination_folder,
                    request.destination_label,
                    "Open folder",
                );
            }
            MessageMoveOutcome::ConfirmedNeedsSync => {
                self.require_move_sync(MoveRecoveryReason::CacheNeedsSync)
            }
        }
    }

    fn show_message_move_completion(
        &mut self,
        destination_folder: FolderReference,
        destination_label: String,
        open_action_label: &'static str,
    ) {
        self.message_detail_state = MessageDetailState::MoveCompleted {
            destination_folder,
            destination_label,
            open_action_label,
        };
    }

    fn require_move_sync(&mut self, reason: MoveRecoveryReason) {
        self.message_detail_state = MessageDetailState::Empty;
        self.message_move_status = MessageMoveStatus::Idle;
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        self.move_recovery_reason = Some(reason);
    }

    fn finish_failed_message_move(&mut self, message_id: MessageId, error: MailWorkflowError) {
        if message_move_outcome_is_unknown(error) {
            self.move_recovery_reason = Some(MoveRecoveryReason::OutcomeUnknown);
            self.message_move_status = MessageMoveStatus::Idle;
        } else {
            self.message_move_status = MessageMoveStatus::Failed { message_id, error };
        }
    }

    fn finish_well_known_folder_action(
        &mut self,
        request: WellKnownFolderActionRequest,
        action_result: Result<WellKnownFolderMoveOutcome, MailWorkflowError>,
    ) {
        self.well_known_folder_action_task = None;
        match action_result {
            Ok(outcome) => self.finish_confirmed_well_known_folder_action(request, outcome),
            Err(error) => self.finish_failed_well_known_folder_action(
                request.message_id,
                request.destination,
                error,
            ),
        }
    }

    fn finish_confirmed_well_known_folder_action(
        &mut self,
        request: WellKnownFolderActionRequest,
        outcome: WellKnownFolderMoveOutcome,
    ) {
        match outcome.cache_outcome() {
            MessageMoveOutcome::ConfirmedAndCached => {
                self.finish_cached_well_known_folder_action(
                    request,
                    outcome.destination_folder().clone(),
                );
            }
            MessageMoveOutcome::ConfirmedNeedsSync => {
                self.require_move_sync(MoveRecoveryReason::CacheNeedsSync);
            }
        }
    }

    fn finish_cached_well_known_folder_action(
        &mut self,
        request: WellKnownFolderActionRequest,
        destination_folder: FolderReference,
    ) {
        self.remove_confirmed_message(&request.message_id);
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        match request.destination {
            WellKnownMailFolder::Archive => self.show_message_move_completion(
                destination_folder,
                "Archive".to_owned(),
                "Open Archive",
            ),
            WellKnownMailFolder::DeletedItems => {
                self.message_detail_state = MessageDetailState::DeleteCompleted;
            }
        }
    }

    fn finish_failed_well_known_folder_action(
        &mut self,
        message_id: MessageId,
        destination: WellKnownMailFolder,
        error: MailWorkflowError,
    ) {
        if message_move_outcome_is_unknown(error) {
            self.move_recovery_reason = Some(MoveRecoveryReason::OutcomeUnknown);
            self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        } else {
            self.well_known_folder_action_status = WellKnownFolderActionStatus::Failed {
                message_id,
                destination,
                error,
            };
        }
    }

    fn remove_confirmed_message(&mut self, message_id: &MessageId) {
        self.remove_message_from_current_folder(message_id);
        self.selected_message_id = None;
    }

    fn remove_message_from_current_folder(&mut self, message_id: &MessageId) {
        let messages = self
            .messages
            .drain(..)
            .filter(|message| message.id() != message_id)
            .collect();
        self.set_messages(messages);
    }

    fn message_move_pending_for(&self, message_id: &MessageId) -> bool {
        matches!(
            &self.message_move_status,
            MessageMoveStatus::Pending {
                message_id: pending_id,
                ..
            } if *pending_id == *message_id
        )
    }

    fn well_known_folder_action_in_progress_for(
        &self,
        message_id: &MessageId,
        destination: WellKnownMailFolder,
    ) -> bool {
        matches!(
            &self.well_known_folder_action_status,
            WellKnownFolderActionStatus::Pending {
                message_id: pending_id,
                destination: pending_destination,
            } if pending_id == message_id && *pending_destination == destination
        )
    }

    fn message_move_status_for(&self, message_id: &MessageId) -> Option<&MessageMoveStatus> {
        match &self.message_move_status {
            MessageMoveStatus::Pending {
                message_id: pending_id,
                ..
            }
            | MessageMoveStatus::Failed {
                message_id: pending_id,
                ..
            } if *pending_id == *message_id => Some(&self.message_move_status),
            _ => None,
        }
    }

    fn well_known_folder_action_status_for(
        &self,
        message_id: &MessageId,
    ) -> Option<&WellKnownFolderActionStatus> {
        match &self.well_known_folder_action_status {
            WellKnownFolderActionStatus::Pending {
                message_id: pending_id,
                ..
            }
            | WellKnownFolderActionStatus::Failed {
                message_id: pending_id,
                ..
            } if pending_id == message_id => Some(&self.well_known_folder_action_status),
            _ => None,
        }
    }

    fn selected_follow_up_flag_request(&self) -> Option<FollowUpFlagUpdateRequest> {
        let folder = self.selected_folder.as_ref()?;
        if !self.can_update_message_in(folder) {
            return None;
        }
        let folder = folder.clone();
        let message_id = self.selected_message_id.clone()?;
        let current_status = self
            .messages
            .iter()
            .find(|message| message.id() == &message_id)?
            .metadata()
            .follow_up_flag_status();
        let requested = match current_status {
            Some(FollowUpFlagStatus::Flagged) => FollowUpFlagStatus::NotFlagged,
            Some(FollowUpFlagStatus::NotFlagged | FollowUpFlagStatus::Complete) | None => {
                FollowUpFlagStatus::Flagged
            }
        };
        Some(FollowUpFlagUpdateRequest {
            folder,
            message_id,
            requested,
        })
    }

    fn finish_follow_up_flag_update(
        &mut self,
        request: FollowUpFlagUpdateRequest,
        flag_update_outcome: Result<FollowUpFlagUpdateOutcome, MailWorkflowError>,
        write_sync_required: bool,
    ) {
        self.follow_up_flag_update = if self.selected_folder.as_ref() == Some(&request.folder) {
            self.selected_folder_write_sync_required = write_sync_required;
            self.apply_follow_up_flag_result(request, flag_update_outcome)
        } else {
            FollowUpFlagUpdateStatus::Idle
        };
        self.follow_up_flag_task = None;
    }

    fn apply_follow_up_flag_result(
        &mut self,
        request: FollowUpFlagUpdateRequest,
        flag_update_outcome: Result<FollowUpFlagUpdateOutcome, MailWorkflowError>,
    ) -> FollowUpFlagUpdateStatus {
        match flag_update_outcome {
            Ok(FollowUpFlagUpdateOutcome::ConfirmedAndCached(status)) => {
                self.apply_follow_up_flag_to_message(&request.message_id, status);
                FollowUpFlagUpdateStatus::Confirmed {
                    message_id: request.message_id,
                    status,
                }
            }
            Ok(FollowUpFlagUpdateOutcome::ConfirmedNeedsSync { status, .. }) => {
                self.apply_follow_up_flag_to_message(&request.message_id, status);
                FollowUpFlagUpdateStatus::ConfirmedNeedsSync {
                    message_id: request.message_id,
                    status,
                }
            }
            Err(error) => FollowUpFlagUpdateStatus::Failed {
                message_id: request.message_id,
                requested: request.requested,
                error,
            },
        }
    }

    fn apply_follow_up_flag_to_message(
        &mut self,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) {
        let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id() == message_id)
        else {
            return;
        };
        let mut patch = MessageMetadataPatch::new();
        patch.set_follow_up_flag_status(Some(status));
        message.apply_patch(patch);
    }

    fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sync_running {
            return;
        }
        let title = if self.pending_send.is_some() {
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
        !self.sync_running
            && !self.message_update_in_progress()
            && self.pending_send.is_none()
            && self.pending_send_error.is_none()
            && !window.has_active_sheet(cx)
    }

    fn selected_reply_target(&self) -> Option<ComposerTarget> {
        let MessageDetailState::Opened(opened) = &self.message_detail_state else {
            return None;
        };
        let message = opened.message();
        if self.selected_message_id.as_ref() != Some(message.id())
            || self.selected_folder.as_ref() != Some(message.folder())
        {
            return None;
        }
        Some(ComposerTarget::Reply {
            source: MailReplySource::new(message.folder().clone(), message.id().clone()),
            subject: message.metadata().subject().map(str::to_owned),
        })
    }

    fn composer_context(&self, target: ComposerTarget) -> MailComposerContext {
        let pending = match self.pending_send_error {
            Some(error) => Err(error),
            None => Ok(self.pending_send.clone()),
        };
        MailComposerContext {
            workflow: Arc::clone(&self.workflow),
            permission_requester: self.permission_requester.clone(),
            account_id: self.profile.id().clone(),
            pending,
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
                    view.pending_send = pending.clone();
                    view.pending_send_error = None;
                    cx.notify();
                }
            },
        )
    }
}

fn message_move_outcome_is_unknown(error: MailWorkflowError) -> bool {
    error
        == MailWorkflowError::Provider(
            einfach_mail_application::MailOperationError::MoveOutcomeUnknown,
        )
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
            .size(px(480.))
            .on_close(move |_, window, cx| {
                if composer_for_close.read(cx).operation_in_progress() {
                    open_composer_sheet(window, cx, title, composer_for_close.clone());
                }
            })
            .child(composer.clone())
    });
}

impl EventEmitter<MailViewEvent> for MailView {}

fn read_state_progress_message(state: ReadState) -> String {
    match state {
        ReadState::Read => "Marking the message as read…".to_owned(),
        ReadState::Unread => "Marking the message as unread…".to_owned(),
    }
}

fn read_state_confirmed_message(state: ReadState) -> &'static str {
    match state {
        ReadState::Read => "Marked read",
        ReadState::Unread => "Marked unread",
    }
}

fn read_state_cache_sync_message(state: ReadState) -> &'static str {
    match state {
        ReadState::Read => {
            "Microsoft marked this message read. Sync mail to update the encrypted cache."
        }
        ReadState::Unread => {
            "Microsoft marked this message unread. Sync mail to update the encrypted cache."
        }
    }
}

fn read_state_failure_message(state: ReadState, error: MailWorkflowError) -> String {
    match error {
        MailWorkflowError::Provider(MailOperationError::ServiceUnavailable) => format!(
            "Couldn’t mark the message {}. Check your connection and try again.",
            read_state_word(state)
        ),
        _ => read_state_failure_detail(error).map_or_else(
            || {
                format!(
                    "Couldn’t mark the message {}. Sync mail and try again.",
                    read_state_word(state)
                )
            },
            str::to_owned,
        ),
    }
}

fn read_state_failure_detail(error: MailWorkflowError) -> Option<&'static str> {
    match error {
        MailWorkflowError::Provider(MailOperationError::AccessDenied) => Some(
            "Microsoft denied the change. Review mail permissions or check your organization's consent policies.",
        ),
        MailWorkflowError::Provider(MailOperationError::SignInRequired) => {
            Some("Your Microsoft sign-in needs attention. Return to Accounts and reconnect.")
        }
        MailWorkflowError::Provider(MailOperationError::RateLimited { .. }) => {
            Some("Microsoft asked Einfach Mail to wait before retrying. Try again later.")
        }
        MailWorkflowError::Provider(MailOperationError::MessageUpdateOutcomeUnknown) => Some(
            "Microsoft may have changed this message's read state, but Einfach Mail could not confirm it. Sync mail before trying again.",
        ),
        _ => None,
    }
}

fn read_state_word(state: ReadState) -> &'static str {
    match state {
        ReadState::Read => "read",
        ReadState::Unread => "unread",
    }
}

fn follow_up_flag_progress_message(status: FollowUpFlagStatus) -> String {
    match status {
        FollowUpFlagStatus::NotFlagged => "Clearing the follow-up flag…".to_owned(),
        FollowUpFlagStatus::Flagged => "Flagging the message for follow-up…".to_owned(),
        FollowUpFlagStatus::Complete => "Marking follow-up complete…".to_owned(),
    }
}

fn follow_up_flag_confirmed_message(status: FollowUpFlagStatus) -> &'static str {
    match status {
        FollowUpFlagStatus::NotFlagged => "Follow-up flag cleared",
        FollowUpFlagStatus::Flagged => "Flagged for follow-up",
        FollowUpFlagStatus::Complete => "Follow-up marked complete",
    }
}

fn follow_up_flag_cache_sync_message(status: FollowUpFlagStatus) -> &'static str {
    match status {
        FollowUpFlagStatus::NotFlagged => {
            "Microsoft cleared the follow-up flag. Sync this folder to update the encrypted cache."
        }
        FollowUpFlagStatus::Flagged => {
            "Microsoft flagged this message. Sync this folder to update the encrypted cache."
        }
        FollowUpFlagStatus::Complete => {
            "Microsoft marked follow-up complete. Sync this folder to update the encrypted cache."
        }
    }
}

fn follow_up_flag_failure_message(status: FollowUpFlagStatus, error: MailWorkflowError) -> String {
    match error {
        MailWorkflowError::Provider(MailOperationError::AccessDenied) => {
            "Microsoft denied the flag change. Review mail permissions or check your organization's consent policies.".to_owned()
        }
        MailWorkflowError::Provider(MailOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and reconnect.".to_owned()
        }
        MailWorkflowError::Provider(MailOperationError::RateLimited { .. }) => {
            "Microsoft asked Einfach Mail to wait before retrying. Try again later.".to_owned()
        }
        MailWorkflowError::Provider(MailOperationError::MessageUpdateOutcomeUnknown) => {
            "Microsoft may have changed this message's follow-up flag, but Einfach Mail could not confirm it. Sync this folder before trying again.".to_owned()
        }
        MailWorkflowError::FolderSyncRequired => {
            "Sync this folder before changing message state again.".to_owned()
        }
        MailWorkflowError::Provider(MailOperationError::ServiceUnavailable) => {
            "Couldn’t update the follow-up flag. Check your connection and try again.".to_owned()
        }
        _ => format!(
            "Couldn’t {} the follow-up flag. Sync this folder and try again.",
            match status {
                FollowUpFlagStatus::NotFlagged => "clear",
                FollowUpFlagStatus::Flagged => "set",
                FollowUpFlagStatus::Complete => "complete",
            }
        ),
    }
}

async fn load_snapshot(
    workflow: &dyn MailSession,
    account_id: &einfach_mail_domain::AccountId,
    preferred: Option<&FolderReference>,
) -> Result<MailSnapshot, MailWorkflowError> {
    let folders = workflow.list_cached_folders(account_id).await?;
    let pending_send = workflow.load_pending_send(account_id).await;
    let selected_folder = preferred
        .filter(|preferred| {
            folders
                .iter()
                .any(|folder| folder.reference() == *preferred)
        })
        .cloned()
        .or_else(|| preferred_folder(&folders));
    let (messages, sync_state, folder_write_sync_required) = if let Some(folder) = &selected_folder
    {
        (
            workflow.list_cached_messages(folder).await?,
            workflow.cached_sync_state(folder).await?,
            workflow.folder_write_sync_required(folder).await?,
        )
    } else {
        (Vec::new(), None, false)
    };
    Ok(MailSnapshot {
        folders,
        selected_folder,
        messages,
        sync_state,
        folder_write_sync_required,
        pending_send,
    })
}

fn preferred_folder(folders: &[MailFolder]) -> Option<FolderReference> {
    folders
        .iter()
        .find(|folder| folder.display_name().eq_ignore_ascii_case("inbox"))
        .or_else(|| folders.first())
        .map(|folder| folder.reference().clone())
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

fn mail_sync_progress_channel() -> (
    impl Fn(MailSyncProgress) + Send + Sync + 'static,
    tokio::sync::watch::Receiver<Option<MailSyncProgress>>,
) {
    let (sender, receiver) = tokio::sync::watch::channel(None);
    let progress_handler = move |progress| {
        sender.send_replace(Some(progress));
    };
    (progress_handler, receiver)
}

async fn synchronize_with_progress(
    workflow: &dyn MailSession,
    account_id: &AccountId,
    sync_started_at: SystemTime,
    mut on_progress: impl FnMut(MailSyncProgress),
) -> Result<MailSyncReport, MailWorkflowError> {
    let (progress_handler, mut progress_receiver) = mail_sync_progress_channel();
    let mut sync_future =
        Box::pin(workflow.synchronize_account(account_id, sync_started_at, &progress_handler));
    loop {
        let changed = tokio::select! {
            biased;
            changed = progress_receiver.changed() => changed,
            sync_result = &mut sync_future => return sync_result,
        };
        if changed.is_err() {
            return sync_future.await;
        }
        if let Some(progress) = *progress_receiver.borrow_and_update() {
            on_progress(progress);
        }
    }
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

mod render;

#[cfg(test)]
mod tests;
