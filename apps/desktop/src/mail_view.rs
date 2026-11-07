use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::mail_composer::{
    ComposerTarget, MailComposer, MailComposerContext, MailComposerEvent, MailPermissionRequester,
};
use einfach_mail_application::{
    FollowUpFlagUpdateOutcome, MailCacheError, MailOperationError, MailSession, MailSyncProgress,
    MailSyncReport, MailSyncState, MailWorkflowError, MessageBodyForDisplay, MessageMoveOutcome,
    OpenedMessage, ReadStateUpdateOutcome, SanitizedEmailHtml, WellKnownFolderMoveOutcome,
    WellKnownMailFolder, allowed_external_message_url,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, FolderReference, FollowUpFlagStatus, MailFolder, MailMessage,
    MailReplySource, MessageId, MessageMetadataPatch, PendingMailSend, ReadState,
};
use gpui_kit::base::{VirtualListScrollHandle, v_virtual_list};
use gpui_kit::component::WindowExt;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Selectable, StyledExt,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, Div, Entity, EventEmitter, FocusHandle, ImageSource,
    IntoElement, KeyBinding, Render, Role, ScrollStrategy, SharedString, Stateful, Subscription,
    Task, WeakEntity, Window, base::TextSelectionLayer, component::text::TextView, div, prelude::*,
    px, size,
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
    message_row_sizes: Rc<Vec<gpui_kit::Size<gpui_kit::Pixels>>>,
    message_list_focus: FocusHandle,
    message_list_scroll: VirtualListScrollHandle,
    sync_state: Option<MailSyncState>,
    sync_status: SyncStatus,
    sync_running: bool,
    sync_task: Option<Task<()>>,
    selected_message_id: Option<MessageId>,
    message_state: MessageState,
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

#[derive(Clone, Copy)]
enum MessageNavigation {
    Previous,
    Next,
    First,
    Last,
}

impl MessageNavigation {
    fn target_index(self, selected_index: Option<usize>, message_count: usize) -> Option<usize> {
        if message_count == 0 {
            return None;
        }

        let last_index = message_count - 1;
        match self {
            Self::Previous => Some(
                selected_index.map_or(last_index, |index| index.min(last_index).saturating_sub(1)),
            ),
            Self::Next => Some(selected_index.map_or(0, |index| {
                index.min(last_index).saturating_add(1).min(last_index)
            })),
            Self::First => Some(0),
            Self::Last => Some(last_index),
        }
    }
}

enum MessageState {
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
            message_row_sizes: Rc::new(Vec::new()),
            message_list_focus: cx.focus_handle().tab_stop(true),
            message_list_scroll: VirtualListScrollHandle::new(),
            sync_state: None,
            sync_status: SyncStatus::LoadingCache,
            sync_running: true,
            sync_task: None,
            selected_message_id: None,
            message_state: MessageState::Empty,
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
        self.message_state = MessageState::Empty;
        self.message_task = None;
        self.read_state_update = ReadStateUpdateStatus::Idle;
        self.follow_up_flag_update = FollowUpFlagUpdateStatus::Idle;
        self.message_move_status = MessageMoveStatus::Idle;
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
    }

    fn set_messages(&mut self, messages: Vec<MailMessage>) {
        self.message_list_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.message_row_sizes = Rc::new(vec![size(px(1.), px(72.)); messages.len()]);
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
        self.message_state = MessageState::Empty;
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
        self.message_state = MessageState::Loading;
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
                        view.message_state = MessageState::Empty;
                    }
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                        view.message_state = MessageState::Failed(error);
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
        self.message_state = MessageState::Loading;
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
                view.message_state = match opened {
                    Ok(message) => MessageState::Opened(Box::new(message)),
                    Err(error) => MessageState::Failed(error),
                };
                view.message_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn navigate_message(&mut self, navigation: MessageNavigation, cx: &mut Context<Self>) {
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
        self.navigate_message(MessageNavigation::Previous, cx);
    }

    fn on_select_next_message(
        &mut self,
        _: &SelectNextMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(MessageNavigation::Next, cx);
    }

    fn on_select_first_message(
        &mut self,
        _: &SelectFirstMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(MessageNavigation::First, cx);
    }

    fn on_select_last_message(
        &mut self,
        _: &SelectLastMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(MessageNavigation::Last, cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.sync_running || self.message_update_in_progress() {
            return;
        }
        self.sync_running = true;
        self.sync_status = SyncStatus::Syncing;
        self.selected_message_id = None;
        self.message_state = MessageState::Empty;
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

    fn message_update_in_progress(&self) -> bool {
        self.read_state_task.is_some()
            || self.follow_up_flag_task.is_some()
            || self.message_move_task.is_some()
            || self.well_known_folder_action_task.is_some()
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
        self.message_state = MessageState::MoveCompleted {
            destination_folder,
            destination_label,
            open_action_label,
        };
    }

    fn require_move_sync(&mut self, reason: MoveRecoveryReason) {
        self.message_state = MessageState::Empty;
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
                self.message_state = MessageState::DeleteCompleted;
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
        let MessageState::Opened(opened) = &self.message_state else {
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

    fn render_reply_action(&self, message_id: &MessageId, cx: &mut Context<Self>) -> AnyElement {
        Button::new("message-reply")
            .debug_selector(|| "message-reply".to_owned())
            .ghost()
            .label("Reply")
            .disabled(
                self.sync_running
                    || self.message_update_in_progress()
                    || self.pending_send.is_some()
                    || self.pending_send_error.is_some()
                    || self.selected_message_id.as_ref() != Some(message_id),
            )
            .on_click(cx.listener(|view, _, window, cx| {
                view.open_reply_composer(window, cx);
            }))
            .into_any_element()
    }

    fn render_header(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.sync_status_label();
        let status_role = if matches!(self.sync_status, SyncStatus::Failed(_)) {
            Role::Alert
        } else {
            Role::Status
        };

        div()
            .w_full()
            .h(px(58.))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(div().text_lg().font_bold().child("Mail"))
                    .child(
                        div()
                            .id("mail-sync-status")
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .role(status_role)
                            .aria_label(status.clone())
                            .child(status),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("new-mail-message")
                            .debug_selector(|| "new-mail-message".to_owned())
                            .primary()
                            .label(if self.pending_send.is_some() {
                                "Resolve send"
                            } else {
                                "Compose"
                            })
                            .disabled(self.sync_running)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.open_composer(window, cx);
                            })),
                    )
                    .child(
                        Button::new("sync-mail")
                            .ghost()
                            .label("Sync")
                            .loading(self.sync_running)
                            .disabled(self.sync_running || self.message_update_in_progress())
                            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                    )
                    .child(
                        Button::new("account")
                            .debug_selector(|| "mail-account-navigation".to_owned())
                            .ghost()
                            .label("Account")
                            .disabled(self.message_update_in_progress())
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(MailViewEvent::BackToAccount);
                            })),
                    ),
            )
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
                    state
                        .last_synchronized_at()
                        .map(|last_sync| format!(" Cached mail from {}.", elapsed_label(last_sync)))
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
                |time| format!("{prefix} · {}", elapsed_label(time)),
            )
    }

    fn render_folder_sidebar(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut folders = div().flex().flex_col().gap_1().overflow_y_scrollbar();
        for folder in &self.folders {
            let reference = folder.reference().clone();
            folders = folders.child(
                Button::new(format!("folder:{}", reference.folder_id().as_str()))
                    .ghost()
                    .selected(self.selected_folder.as_ref() == Some(&reference))
                    .label(folder.display_name().to_owned())
                    .disabled(self.sync_running || self.message_update_in_progress())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.select_folder(reference.clone(), cx);
                    })),
            );
        }
        div()
            .w(px(216.))
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .bg(colors.surface)
            .child(div().text_sm().font_bold().child("Folders"))
            .child(folders.flex_1().min_h_0())
    }

    fn render_message_list(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        if let MessageState::Failed(error) = &self.message_state {
            return div()
                .w(px(344.))
                .min_h_0()
                .p_4()
                .id("mail-list-error")
                .role(Role::Alert)
                .child(mail_error_message(*error))
                .into_any_element();
        }
        let Some(folder) = self.selected_folder.as_ref() else {
            return div()
                .w(px(344.))
                .min_h_0()
                .p_4()
                .text_color(colors.muted_foreground)
                .child("No visible mail folders were found.")
                .into_any_element();
        };
        if self.messages.is_empty() {
            let message = if matches!(self.message_state, MessageState::Loading) {
                "Loading messages…"
            } else {
                "No messages in this folder."
            };
            return div()
                .w(px(344.))
                .min_h_0()
                .p_4()
                .text_color(colors.muted_foreground)
                .child(message)
                .into_any_element();
        }

        let folder_reference = folder.clone();
        let sizes = Rc::clone(&self.message_row_sizes);
        div()
            .w(px(344.))
            .min_h_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.border)
            .child(div().px_4().py_3().text_sm().font_bold().child("Messages"))
            .child({
                let message_list = div().id("cached-message-list").flex_1().min_h_0();
                #[cfg(test)]
                let message_list = message_list.test_support();
                message_list
                    .track_focus(&self.message_list_focus)
                    .key_context(MESSAGE_LIST_CONTEXT)
                    .role(Role::ListBox)
                    .aria_label("Messages")
                    .on_action(cx.listener(Self::on_select_previous_message))
                    .on_action(cx.listener(Self::on_select_next_message))
                    .on_action(cx.listener(Self::on_select_first_message))
                    .on_action(cx.listener(Self::on_select_last_message))
                    .child(
                        v_virtual_list(
                            cx.entity(),
                            "cached-message-options",
                            sizes,
                            move |view, range, _, cx| {
                                let list_size = view.messages.len();
                                range
                                    .filter_map(|index| {
                                        let message = view.messages.get(index)?;
                                        let message_id = message.id().clone();
                                        let selected =
                                            view.selected_message_id.as_ref() == Some(&message_id);
                                        let sender = message_sender(message);
                                        let subject = message
                                            .metadata()
                                            .subject()
                                            .unwrap_or("(No subject)")
                                            .to_owned();
                                        let accessible_label = format!("{sender}. {subject}");
                                        let expected_folder = folder_reference.clone();
                                        let option = div()
                                            .id(format!("message-option-{}", message_id.as_str()))
                                            .debug_selector(|| {
                                                format!("message-row-{}", message_id.as_str())
                                            })
                                            .w_full()
                                            .h(px(72.))
                                            .px_3()
                                            .py_3()
                                            .rounded_md()
                                            .text_color(colors.foreground)
                                            .role(Role::ListBoxOption)
                                            .aria_label(if view.message_update_in_progress() {
                                                format!("{accessible_label}, mail action pending")
                                            } else {
                                                accessible_label
                                            })
                                            .aria_position_in_set(index + 1)
                                            .aria_size_of_set(list_size)
                                            .aria_selected(selected)
                                            .when(selected, |option| {
                                                option
                                                    .aria_active_descendant()
                                                    .bg(colors.accent)
                                                    .text_color(colors.accent_foreground)
                                            })
                                            .hover(|option| {
                                                option.bg(if selected {
                                                    colors.accent
                                                } else {
                                                    colors.muted
                                                })
                                            })
                                            .child(
                                                div()
                                                    .w_full()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .text_sm()
                                                            .font_bold()
                                                            .child(sender),
                                                    )
                                                    .child(div().w_full().text_sm().child(subject)),
                                            )
                                            .when(!view.message_update_in_progress(), |option| {
                                                option.on_click(cx.listener(
                                                    move |view, _, window, cx| {
                                                        view.select_message_from_option(
                                                            message_id.clone(),
                                                            &expected_folder,
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                            });
                                        #[cfg(test)]
                                        let option = option.test_support();
                                        Some(option)
                                    })
                                    .collect()
                            },
                        )
                        .track_scroll(&self.message_list_scroll),
                    )
                    .focus_visible(|style| style.border_1().border_color(colors.ring))
            })
            .into_any_element()
    }

    fn render_message_detail(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match &self.message_state {
            MessageState::Empty => {
                let panel = div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(colors.muted_foreground);
                if let Some(notice) = self.render_move_recovery_notice(cx) {
                    panel.child(notice).into_any_element()
                } else {
                    panel
                        .child("Select a message to read it.")
                        .into_any_element()
                }
            }
            MessageState::Loading => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .items_center()
                .justify_center()
                .text_color(colors.muted_foreground)
                .child("Loading message…")
                .into_any_element(),
            MessageState::Failed(error) => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .p_6()
                .id("mail-message-error")
                .role(Role::Alert)
                .child(mail_error_message(*error))
                .children(self.render_move_recovery_notice(cx))
                .into_any_element(),
            MessageState::Opened(opened) => {
                let metadata = opened.message().metadata();
                let message_id = opened.message().id();
                let body = render_message_body(opened.body(), colors);
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_6()
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(div().text_lg().font_bold().child(
                                        metadata.subject().unwrap_or("(No subject)").to_owned(),
                                    ))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(self.render_read_state_action(message_id, cx))
                                            .child(
                                                self.render_follow_up_flag_action(message_id, cx),
                                            )
                                            .child(self.render_well_known_folder_action(
                                                WellKnownMailFolder::Archive,
                                                message_id,
                                                cx,
                                            ))
                                            .child(self.render_move_action(message_id, cx))
                                            .child(self.render_reply_action(message_id, cx))
                                            .child(self.render_well_known_folder_action(
                                                WellKnownMailFolder::DeletedItems,
                                                message_id,
                                                cx,
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(colors.muted_foreground)
                                    .child(message_sender(opened.message())),
                            )
                            .children(self.read_state_update.feedback_for(message_id).map(
                                |feedback| self.render_read_state_feedback_element(feedback, cx),
                            ))
                            .children(self.render_follow_up_flag_feedback(message_id, cx))
                            .children(self.render_message_move_feedback(message_id, cx))
                            .children(self.render_well_known_folder_action_feedback(message_id, cx))
                            .children(self.render_move_recovery_notice(cx))
                            .children(self.render_read_state_reconciliation_notice()),
                    )
                    .child(body)
                    .into_any_element()
            }
            MessageState::MoveCompleted {
                destination_folder,
                destination_label,
                open_action_label,
            } => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .id("message-move-completed")
                .debug_selector(|| "message-move-completed".to_owned())
                .role(Role::Status)
                .child(format!("Moved to {destination_label}."))
                .child(
                    Button::new("open-moved-message-folder")
                        .debug_selector(|| "open-moved-message-folder".to_owned())
                        .label(*open_action_label)
                        .on_click(cx.listener({
                            let destination_folder = destination_folder.clone();
                            move |view, _, _, cx| {
                                view.select_folder(destination_folder.clone(), cx);
                            }
                        })),
                )
                .into_any_element(),
            MessageState::DeleteCompleted => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .id("message-delete-completed")
                .debug_selector(|| "message-delete-completed".to_owned())
                .role(Role::Status)
                .child("Moved to Deleted Items.")
                .into_any_element(),
        }
    }

    fn render_move_action(&self, message_id: &MessageId, cx: &mut Context<Self>) -> AnyElement {
        let Some(source_folder) = self.selected_folder.as_ref() else {
            return Button::new("message-move")
                .label("Move to…")
                .disabled(true)
                .into_any_element();
        };
        let destinations = self.move_destinations(source_folder);
        let selected_message_id = message_id.clone();
        let entity = cx.entity().downgrade();
        let pending = self.message_move_pending_for(message_id);
        Button::new("message-move")
            .ghost()
            .debug_selector(|| "message-move".to_owned())
            .label(if pending { "Moving…" } else { "Move to…" })
            .loading(pending)
            .disabled(!self.message_move_allowed() || destinations.is_empty())
            .dropdown_menu(move |menu, _, _| {
                build_message_move_menu(
                    menu,
                    destinations.clone(),
                    entity.clone(),
                    selected_message_id.clone(),
                )
            })
            .into_any_element()
    }

    fn render_well_known_folder_action(
        &self,
        destination: WellKnownMailFolder,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pending = self.well_known_folder_action_in_progress_for(message_id, destination);
        let (button_id, label, pending_label) = match destination {
            WellKnownMailFolder::Archive => ("message-archive", "Archive", "Archiving…"),
            WellKnownMailFolder::DeletedItems => ("message-delete", "Delete", "Deleting…"),
        };
        let button = Button::new(button_id)
            .debug_selector(|| button_id.to_owned())
            .label(if pending { pending_label } else { label })
            .loading(pending)
            .disabled(
                !self.message_move_allowed()
                    || self.selected_message_id.as_ref() != Some(message_id),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                view.start_well_known_folder_action(destination, cx);
            }));
        match destination {
            WellKnownMailFolder::Archive => button.ghost().into_any_element(),
            WellKnownMailFolder::DeletedItems => button.danger().into_any_element(),
        }
    }

    fn move_destinations(&self, source_folder: &FolderReference) -> Vec<(FolderReference, String)> {
        self.folders
            .iter()
            .filter(|folder| {
                folder.reference() != source_folder
                    && folder.reference().account_id() == source_folder.account_id()
            })
            .map(|folder| (folder.reference().clone(), self.folder_menu_label(folder)))
            .collect()
    }

    fn folder_menu_label(&self, folder: &MailFolder) -> String {
        let mut labels = vec![folder.display_name()];
        let mut parent = folder.parent();
        let mut remaining = self.folders.len();
        while let Some(parent_reference) = parent {
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            let Some(parent_folder) = self
                .folders
                .iter()
                .find(|candidate| candidate.reference() == parent_reference)
            else {
                break;
            };
            labels.push(parent_folder.display_name());
            parent = parent_folder.parent();
        }
        labels.reverse();
        labels.join(" / ")
    }

    fn render_message_move_feedback(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match self.message_move_status_for(message_id)? {
            MessageMoveStatus::Pending {
                destination_label, ..
            } => Some(render_message_pending_feedback(
                "message-move-pending",
                format!("Moving to {destination_label}…"),
            )),
            MessageMoveStatus::Failed { error, .. } => Some(self.render_message_action_error(
                "message-move-error",
                "review-permissions-from-move",
                *error,
                cx,
            )),
            _ => None,
        }
    }

    fn render_well_known_folder_action_feedback(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match self.well_known_folder_action_status_for(message_id)? {
            WellKnownFolderActionStatus::Pending { destination, .. } => {
                Some(self.render_well_known_folder_action_pending(*destination))
            }
            WellKnownFolderActionStatus::Failed {
                destination, error, ..
            } => Some(self.render_well_known_folder_action_error(*destination, *error, cx)),
            WellKnownFolderActionStatus::Idle => None,
        }
    }

    fn render_well_known_folder_action_pending(
        &self,
        destination: WellKnownMailFolder,
    ) -> AnyElement {
        let pending_id = match destination {
            WellKnownMailFolder::Archive => "message-archive-pending",
            WellKnownMailFolder::DeletedItems => "message-delete-pending",
        };
        render_message_pending_feedback(
            pending_id,
            well_known_folder_progress_message(destination).to_owned(),
        )
    }

    fn render_well_known_folder_action_error(
        &self,
        destination: WellKnownMailFolder,
        error: MailWorkflowError,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (error_id, permission_button_id) = match destination {
            WellKnownMailFolder::Archive => {
                ("message-archive-error", "review-permissions-from-archive")
            }
            WellKnownMailFolder::DeletedItems => {
                ("message-delete-error", "review-permissions-from-delete")
            }
        };
        self.render_message_action_error(error_id, permission_button_id, error, cx)
    }

    fn render_message_action_error(
        &self,
        error_id: &'static str,
        permission_button_id: &'static str,
        error: MailWorkflowError,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let feedback = div()
            .id(error_id)
            .debug_selector(|| error_id.to_owned())
            .role(Role::Alert)
            .flex()
            .flex_col()
            .gap_2()
            .child(mail_error_message(error));
        if error
            == MailWorkflowError::Provider(
                einfach_mail_application::MailOperationError::AccessDenied,
            )
        {
            feedback
                .child(
                    Button::new(permission_button_id)
                        .ghost()
                        .debug_selector(|| permission_button_id.to_owned())
                        .label("Review permissions…")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(MailViewEvent::ReviewMailWritePermission);
                        })),
                )
                .into_any_element()
        } else {
            feedback.into_any_element()
        }
    }

    fn render_move_recovery_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reason = self.move_recovery_reason?;
        if self.sync_running && matches!(reason, MoveRecoveryReason::InitialSyncRequired) {
            return None;
        }
        Some(self.render_move_recovery_notice_element(reason, cx))
    }

    fn render_move_recovery_notice_element(
        &self,
        reason: MoveRecoveryReason,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("message-move-recovery")
            .debug_selector(|| "message-move-recovery".to_owned())
            .role(Role::Alert)
            .flex()
            .flex_col()
            .gap_2()
            .child(move_recovery_message(reason))
            .child(
                Button::new("sync-after-message-move")
                    .debug_selector(|| "sync-after-message-move".to_owned())
                    .ghost()
                    .label("Sync mail")
                    .disabled(self.sync_running)
                    .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
            )
            .into_any_element()
    }

    fn render_read_state_action(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let read_state = self.cached_read_state(message_id);
        let pending = self.read_state_update.is_pending_for(message_id);
        let label = match (pending, read_state) {
            (true, _) => "Updating…",
            (false, Some(ReadState::Read)) => "Mark unread",
            (false, Some(ReadState::Unread)) => "Mark read",
            (false, None) => "Mark read",
        };
        Button::new("message-read-state")
            .ghost()
            .debug_selector(|| "message-read-state".to_owned())
            .label(label)
            .loading(pending)
            .disabled(self.read_state_action_disabled(read_state))
            .on_click(cx.listener(|view, _, _, cx| view.update_selected_read_state(cx)))
            .into_any_element()
    }

    fn render_follow_up_flag_action(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let flag_status = self
            .messages
            .iter()
            .find(|message| message.id() == message_id)
            .and_then(|message| message.metadata().follow_up_flag_status());
        Button::new("message-follow-up-flag")
            .ghost()
            .debug_selector(|| "message-follow-up-flag".to_owned())
            .label(
                self.follow_up_flag_update
                    .action_label(message_id, flag_status),
            )
            .loading(self.follow_up_flag_update.is_pending_for(message_id))
            .disabled(self.follow_up_flag_action_disabled(message_id))
            .on_click(cx.listener(|view, _, _, cx| view.update_selected_follow_up_flag(cx)))
            .into_any_element()
    }

    fn follow_up_flag_action_disabled(&self, message_id: &MessageId) -> bool {
        self.selected_message_id.as_ref() != Some(message_id)
            || self
                .selected_folder
                .as_ref()
                .is_none_or(|folder| !self.can_update_message_in(folder))
    }

    fn cached_read_state(&self, message_id: &MessageId) -> Option<ReadState> {
        self.messages
            .iter()
            .find(|message| message.id() == message_id)
            .map(|message| message.metadata().read_state())
    }

    fn read_state_action_disabled(&self, read_state: Option<ReadState>) -> bool {
        let Some(folder) = self.selected_folder.as_ref() else {
            return true;
        };
        read_state.is_none() || !self.can_update_message_in(folder)
    }

    fn can_update_message_in(&self, folder: &FolderReference) -> bool {
        !self.sync_running
            && !self.message_update_in_progress()
            && !matches!(self.sync_status, SyncStatus::Failed(_))
            && self.selected_folder.as_ref() == Some(folder)
            && !self.selected_folder_write_sync_required
    }

    fn render_read_state_feedback_element(
        &self,
        feedback: ReadStateFeedback,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (message, role, show_permission_review) = feedback.into_parts();
        let feedback_view = self.read_state_feedback_view(message, role, cx);
        if show_permission_review {
            feedback_view
                .flex()
                .flex_col()
                .gap_2()
                .child(self.render_permission_review_button(cx))
                .into_any_element()
        } else {
            feedback_view.into_any_element()
        }
    }

    fn read_state_feedback_view(
        &self,
        message: String,
        role: Role,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id("message-read-state-feedback")
            .debug_selector(|| "message-read-state-feedback".to_owned())
            .role(role)
            .text_sm()
            .text_color(if role == Role::Alert {
                colors.destructive
            } else {
                colors.muted_foreground
            })
            .child(message)
    }

    fn render_permission_review_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("review-mail-permissions-from-message")
            .ghost()
            .debug_selector(|| "review-mail-permissions-from-message".to_owned())
            .label("Review mail permissions…")
            .on_click(cx.listener(|_, _, _, cx| {
                cx.emit(MailViewEvent::ReviewMailWritePermission);
            }))
            .into_any_element()
    }

    fn render_follow_up_flag_feedback(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (message, role) = match &self.follow_up_flag_update {
            FollowUpFlagUpdateStatus::Idle => return None,
            FollowUpFlagUpdateStatus::Pending {
                message_id: pending_id,
                requested,
            } if pending_id == message_id => {
                (follow_up_flag_progress_message(*requested), Role::Status)
            }
            FollowUpFlagUpdateStatus::Confirmed {
                message_id: confirmed_id,
                status,
            } if confirmed_id == message_id => (
                follow_up_flag_confirmed_message(*status).to_owned(),
                Role::Status,
            ),
            FollowUpFlagUpdateStatus::ConfirmedNeedsSync {
                message_id: confirmed_id,
                status,
            } if confirmed_id == message_id => (
                follow_up_flag_cache_sync_message(*status).to_owned(),
                Role::Status,
            ),
            FollowUpFlagUpdateStatus::Failed {
                message_id: failed_id,
                requested,
                error,
            } if failed_id == message_id => (
                follow_up_flag_failure_message(*requested, *error),
                Role::Alert,
            ),
            _ => return None,
        };
        let colors = cx.theme().semantic_tokens().colors;
        let feedback = div()
            .id("message-follow-up-flag-feedback")
            .debug_selector(|| "message-follow-up-flag-feedback".to_owned())
            .role(role)
            .text_sm()
            .text_color(if role == Role::Alert {
                colors.destructive
            } else {
                colors.muted_foreground
            })
            .child(message);
        if matches!(
            &self.follow_up_flag_update,
            FollowUpFlagUpdateStatus::Failed {
                message_id: failed_id,
                error: MailWorkflowError::Provider(MailOperationError::AccessDenied),
                ..
            } if failed_id == message_id
        ) {
            Some(
                feedback
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Button::new("review-mail-permissions-from-flag")
                            .ghost()
                            .debug_selector(|| "review-mail-permissions-from-flag".to_owned())
                            .label("Review mail permissions…")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(MailViewEvent::ReviewMailWritePermission);
                            })),
                    )
                    .into_any_element(),
            )
        } else {
            Some(feedback.into_any_element())
        }
    }

    fn render_read_state_reconciliation_notice(&self) -> Option<AnyElement> {
        let message = self.read_state_reconciliation_message()?;
        Some(
            div()
                .id("read-state-reconciliation")
                .debug_selector(|| "read-state-reconciliation".to_owned())
                .role(Role::Alert)
                .text_sm()
                .child(message)
                .into_any_element(),
        )
    }

    fn read_state_reconciliation_message(&self) -> Option<&'static str> {
        self.selected_folder.as_ref()?;
        if self.selected_folder_write_sync_required {
            Some(
                "A message change in this folder needs syncing. Sync mail before changing read state, follow-up flags, moving, archiving, or deleting messages again.",
            )
        } else if matches!(self.sync_status, SyncStatus::Failed(_)) {
            Some(
                "Mail sync failed. Retry sync before changing read state, follow-up flags, moving, archiving, or deleting messages.",
            )
        } else {
            None
        }
    }
}

fn build_message_move_menu(
    menu: PopupMenu,
    destinations: Vec<(FolderReference, String)>,
    entity: WeakEntity<MailView>,
    selected_message_id: MessageId,
) -> PopupMenu {
    destinations
        .into_iter()
        .fold(menu.scrollable(true), |menu, destination| {
            menu.item(message_move_menu_item(
                destination,
                entity.clone(),
                selected_message_id.clone(),
            ))
        })
}

fn message_move_menu_item(
    (destination_folder, destination_label): (FolderReference, String),
    entity: WeakEntity<MailView>,
    selected_message_id: MessageId,
) -> PopupMenuItem {
    let action = MessageMoveMenuAction {
        selected_message_id,
        destination_folder,
        destination_label: destination_label.clone(),
    };
    PopupMenuItem::new(destination_label).on_click(move |_, _, cx| {
        if let Some(view) = entity.upgrade() {
            view.update(cx, |view, cx| {
                view.apply_message_move_menu_action(action.clone(), cx)
            });
        }
    })
}

fn move_recovery_message(reason: MoveRecoveryReason) -> &'static str {
    match reason {
        MoveRecoveryReason::InitialSyncRequired => {
            "Sync mail successfully before archiving, moving, or deleting messages in this session."
        }
        MoveRecoveryReason::OutcomeUnknown => {
            "The message may have moved. Sync mail before trying again."
        }
        MoveRecoveryReason::CacheNeedsSync => {
            "The message move was confirmed, but the local cache needs synchronization."
        }
    }
}

fn message_move_outcome_is_unknown(error: MailWorkflowError) -> bool {
    error
        == MailWorkflowError::Provider(
            einfach_mail_application::MailOperationError::MoveOutcomeUnknown,
        )
}

fn render_message_pending_feedback(selector: &'static str, message: String) -> AnyElement {
    div()
        .id(selector)
        .debug_selector(|| selector.to_owned())
        .role(Role::Status)
        .child(message)
        .into_any_element()
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

fn render_message_body(body: &MessageBodyForDisplay, colors: ColorTokens) -> AnyElement {
    match body {
        MessageBodyForDisplay::PlainText(text) => render_plain_text_body(text),
        MessageBodyForDisplay::SanitizedHtml(html) => render_sanitized_html_body(html),
        MessageBodyForDisplay::PlainTextFallback(content) => {
            render_plain_text_fallback(content, colors)
        }
        MessageBodyForDisplay::HtmlTooLarge => render_oversized_html_fallback(colors),
    }
}

fn render_plain_text_body(text: &Arc<str>) -> AnyElement {
    div()
        .id("mail-message-plain-body")
        .flex_1()
        .min_h_0()
        .overflow_y_scrollbar()
        .p_6()
        .child(SharedString::from(Arc::clone(text)))
        .into_any_element()
}

fn render_plain_text_fallback(content: &SanitizedEmailHtml, colors: ColorTokens) -> AnyElement {
    div()
        .id("mail-message-html-fallback")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .p_6()
        .child(render_plain_text_fallback_status(colors))
        .child(render_plain_text_fallback_content(content))
        .into_any_element()
}

fn render_plain_text_fallback_status(colors: ColorTokens) -> AnyElement {
    div()
        .id("mail-message-html-fallback-status")
        .debug_selector(|| "mail-message-html-fallback-status".into())
        .text_color(colors.muted_foreground)
        .role(Role::Status)
        .child(
            "Rich formatting was removed. Only the beginning of this message is shown as plain text.",
        )
        .into_any_element()
}

fn render_plain_text_fallback_content(content: &SanitizedEmailHtml) -> AnyElement {
    div()
        .id("mail-message-html-fallback-body")
        .debug_selector(|| "mail-message-html-fallback-body".into())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scrollbar()
        .child(TextSelectionLayer)
        .child(
            TextView::html(
                "mail-message-html-fallback-content",
                content.shared_content(),
            )
            .selectable(true)
            .image_source(|_| ImageSource::Custom(Arc::new(|_, _| None)))
            .on_link_click(|_, _, _, _| {}),
        )
        .into_any_element()
}

fn render_sanitized_html_body(html: &SanitizedEmailHtml) -> AnyElement {
    div()
        .id("mail-message-html-body")
        .debug_selector(|| "mail-message-html-body".into())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scrollbar()
        .p_6()
        .child(TextSelectionLayer)
        .child(
            TextView::html("mail-message-html-content", html.shared_content())
                .selectable(true)
                .image_source(|_| ImageSource::Custom(Arc::new(|_, _| None)))
                .on_link_click(|href, event, _, cx| {
                    if event.is_secondary() {
                        return;
                    }
                    if let Some(url) = allowed_external_message_url(href.as_ref()) {
                        cx.open_url(url.as_str());
                    }
                }),
        )
        .into_any_element()
}

fn render_oversized_html_fallback(colors: ColorTokens) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .p_6()
        .text_color(colors.muted_foreground)
        .id("mail-message-body-too-large")
        .debug_selector(|| "mail-message-body-too-large".into())
        .role(Role::Status)
        .child("This message is too large to display safely.")
        .into_any_element()
}

impl EventEmitter<MailViewEvent> for MailView {}

impl Render for MailView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_header(colors, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_folder_sidebar(colors, cx))
                    .child(self.render_message_list(colors, cx))
                    .child(self.render_message_detail(colors, cx)),
            )
    }
}

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

fn elapsed_label(last_sync: SystemTime) -> String {
    let elapsed = SystemTime::now()
        .duration_since(last_sync)
        .unwrap_or(Duration::ZERO);
    match elapsed.as_secs() {
        seconds if seconds < 60 => "less than a minute ago".to_owned(),
        seconds if seconds < 3_600 => format!("{} min ago", seconds / 60),
        seconds if seconds < 86_400 => format!("{} hr ago", seconds / 3_600),
        seconds => format!("{} days ago", seconds / 86_400),
    }
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
        MailWorkflowError::AccountSyncRequired => {
            "Sync mail successfully before archiving, moving, or deleting messages in this session."
        }
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

fn well_known_folder_progress_message(folder: WellKnownMailFolder) -> &'static str {
    match folder {
        WellKnownMailFolder::Archive => "Moving message to Archive…",
        WellKnownMailFolder::DeletedItems => "Moving message to Deleted Items…",
    }
}

#[cfg(test)]
mod tests {
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
        AccountOperationError, MailCache, MailCacheError, MailOperationError, MailProvider,
        MailSendProviderError, MailSyncState, MailWorkflow, MailWorkflowError,
        MessageDeltaPageResponse, MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
    };
    use einfach_mail_domain::{
        AccountId, AccountProfile, FolderId, FolderReference, FollowUpFlagStatus, MailDraft,
        MailFolder, MailMessage, MailReplyDraft, MailSendOperationId, Mailbox, MessageBody,
        MessageBodyType, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageId,
        MessageMetadataPatch, PendingMailSend, ReadState,
    };
    use gpui_kit::base::TextSelection;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::test::TestQueryExt as _;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AppContext, Modifiers, MouseButton, ScrollStrategy, TestAppContext, VisualTestContext,
        point, px,
    };

    use super::{
        FollowUpFlagUpdateStatus, MailPermissionRequester, MailSyncProgress, MailView,
        MailViewEvent, MessageBodyForDisplay, MessageMoveMenuAction, MessageNavigation,
        MessageState, MoveRecoveryReason, ReadStateUpdateStatus, SyncStatus,
        WellKnownFolderActionStatus, bind_message_navigation_keys, follow_up_flag_failure_message,
        mail_error_message, read_state_failure_message,
    };
    use crate::mail_composer::bind_mail_composer_keys;

    #[test]
    fn active_mail_operation_explains_when_to_retry() {
        assert_eq!(
            mail_error_message(MailWorkflowError::OperationInProgress),
            "Another mail operation is active. Try again shortly."
        );
    }

    #[test]
    fn stale_sync_checkpoint_explains_how_to_resume() {
        assert_eq!(
            mail_error_message(MailWorkflowError::Cache(MailCacheError::SyncStateChanged)),
            "Another sync updated this folder. Choose Sync to continue."
        );
    }

    #[test]
    fn follow_up_flag_action_label_matches_cached_state() {
        let message_id = MessageId::try_from("synthetic-message".to_owned())
            .expect("synthetic message ID is valid");
        let pending = FollowUpFlagUpdateStatus::Pending {
            message_id: message_id.clone(),
            requested: FollowUpFlagStatus::Flagged,
        };
        let idle = FollowUpFlagUpdateStatus::Idle;

        assert_eq!(
            pending.action_label(&message_id, Some(FollowUpFlagStatus::Complete)),
            "Updating…"
        );
        assert_eq!(
            idle.action_label(&message_id, Some(FollowUpFlagStatus::Flagged)),
            "Clear flag"
        );
        assert_eq!(
            idle.action_label(&message_id, Some(FollowUpFlagStatus::Complete)),
            "Reopen flag"
        );
        assert_eq!(
            idle.action_label(&message_id, Some(FollowUpFlagStatus::NotFlagged)),
            "Flag"
        );
        assert_eq!(idle.action_label(&message_id, None), "Flag");
    }

    #[derive(Clone)]
    struct OfflineProvider {
        body_fetches: Arc<AtomicUsize>,
    }

    impl MailProvider for OfflineProvider {
        fn list_folders(
            &self,
            _: &AccountId,
        ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn list_messages(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_
        {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn message_delta_page(
            &self,
            _: &FolderReference,
            _: MessageDeltaRequest,
        ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_
        {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn fetch_message_body(
            &self,
            _: &FolderReference,
            _: &MessageId,
        ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
            self.body_fetches.fetch_add(1, Ordering::Relaxed);
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn set_message_read_state(
            &self,
            _: &FolderReference,
            _: &MessageId,
            _: ReadState,
        ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn set_message_follow_up_flag_status(
            &self,
            _: &FolderReference,
            _: &MessageId,
            _: FollowUpFlagStatus,
        ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_
        {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn move_message(
            &self,
            _: &FolderReference,
            _: &MessageId,
            _: &FolderReference,
        ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn well_known_folder(
            &self,
            account_id: &AccountId,
            folder: WellKnownMailFolder,
        ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
            let folder_id = match folder {
                WellKnownMailFolder::Archive => "archive",
                WellKnownMailFolder::DeletedItems => "deleted-items",
            };
            ready(Ok(FolderReference::new(
                account_id.clone(),
                FolderId::try_from(folder_id.to_owned())
                    .expect("synthetic well-known folder ID is valid"),
            )))
        }

        fn send_mail(
            &self,
            _: &AccountId,
            _: MailDraft,
        ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
            ready(Err(MailSendProviderError::ServiceUnavailable))
        }

        fn reply_to_message(
            &self,
            _: &AccountId,
            _: MailReplyDraft,
        ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
            ready(Err(MailSendProviderError::ServiceUnavailable))
        }
    }

    #[derive(Clone, Copy)]
    enum ReadStateActionResponse {
        Confirm,
        Unconfirmed,
        Pending,
        Deny,
        Offline,
    }

    struct ReadStateProvider {
        response: ReadStateActionResponse,
        updates: Arc<Mutex<Vec<(MessageId, ReadState)>>>,
        flag_updates: Arc<Mutex<Vec<(MessageId, FollowUpFlagStatus)>>>,
        sync_failure: bool,
        offline: OfflineProvider,
    }

    impl ReadStateProvider {
        fn new(
            response: ReadStateActionResponse,
            updates: Arc<Mutex<Vec<(MessageId, ReadState)>>>,
        ) -> Self {
            Self {
                response,
                updates,
                flag_updates: Arc::new(Mutex::new(Vec::new())),
                sync_failure: false,
                offline: OfflineProvider {
                    body_fetches: Arc::new(AtomicUsize::new(0)),
                },
            }
        }

        fn with_flag_updates(
            mut self,
            updates: Arc<Mutex<Vec<(MessageId, FollowUpFlagStatus)>>>,
        ) -> Self {
            self.flag_updates = updates;
            self
        }

        fn with_sync_failure(mut self) -> Self {
            self.sync_failure = true;
            self
        }
    }

    impl MailProvider for ReadStateProvider {
        fn list_folders(
            &self,
            account_id: &AccountId,
        ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
            let account_id = account_id.clone();
            let sync_failure = self.sync_failure;
            async move {
                if sync_failure {
                    return Err(MailOperationError::ServiceUnavailable);
                }
                let reference = FolderReference::new(
                    account_id,
                    FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
                );
                Ok(vec![
                    MailFolder::new(reference, None, "Inbox".to_owned())
                        .expect("synthetic folder is valid"),
                ])
            }
        }

        fn list_messages(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_
        {
            ready(Ok(Vec::new()))
        }

        fn message_delta_page(
            &self,
            _: &FolderReference,
            _: MessageDeltaRequest,
        ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_
        {
            let sync_failure = self.sync_failure;
            async move {
                if sync_failure {
                    return Err(MailOperationError::ServiceUnavailable);
                }
                let page = MessageDeltaPage::new(
                    Vec::new(),
                    MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/delta?token=synthetic".to_owned(),
                    )),
                );
                Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                    page, 0,
                )))
            }
        }

        fn fetch_message_body(
            &self,
            _: &FolderReference,
            _: &MessageId,
        ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn set_message_read_state(
            &self,
            _: &FolderReference,
            message_id: &MessageId,
            requested: ReadState,
        ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
            let message_id = message_id.clone();
            let response = self.response;
            let updates = Arc::clone(&self.updates);
            async move {
                updates
                    .lock()
                    .expect("fixture read-state log mutex is healthy")
                    .push((message_id, requested));
                match response {
                    ReadStateActionResponse::Confirm => Ok(requested),
                    ReadStateActionResponse::Unconfirmed => {
                        Err(MailOperationError::MessageUpdateOutcomeUnknown)
                    }
                    ReadStateActionResponse::Pending => {
                        pending::<Result<ReadState, MailOperationError>>().await
                    }
                    ReadStateActionResponse::Deny => Err(MailOperationError::AccessDenied),
                    ReadStateActionResponse::Offline => Err(MailOperationError::ServiceUnavailable),
                }
            }
        }

        fn set_message_follow_up_flag_status(
            &self,
            _: &FolderReference,
            message_id: &MessageId,
            status: FollowUpFlagStatus,
        ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_
        {
            let message_id = message_id.clone();
            let updates = Arc::clone(&self.flag_updates);
            let response = self.response;
            async move {
                updates
                    .lock()
                    .expect("fixture flag-update log mutex is healthy")
                    .push((message_id, status));
                match response {
                    ReadStateActionResponse::Confirm => Ok(status),
                    ReadStateActionResponse::Unconfirmed => {
                        Err(MailOperationError::MessageUpdateOutcomeUnknown)
                    }
                    ReadStateActionResponse::Pending => {
                        pending::<Result<FollowUpFlagStatus, MailOperationError>>().await
                    }
                    ReadStateActionResponse::Deny => Err(MailOperationError::AccessDenied),
                    ReadStateActionResponse::Offline => Err(MailOperationError::ServiceUnavailable),
                }
            }
        }

        fn move_message(
            &self,
            _: &FolderReference,
            _: &MessageId,
            _: &FolderReference,
        ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn well_known_folder(
            &self,
            account_id: &AccountId,
            folder: WellKnownMailFolder,
        ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
            self.offline.well_known_folder(account_id, folder)
        }

        fn send_mail(
            &self,
            account_id: &AccountId,
            draft: MailDraft,
        ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
            self.offline.send_mail(account_id, draft)
        }

        fn reply_to_message(
            &self,
            account_id: &AccountId,
            draft: MailReplyDraft,
        ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
            self.offline.reply_to_message(account_id, draft)
        }
    }

    #[derive(Clone, Copy)]
    enum MessageMoveActionResponse {
        Confirm,
        AccessDenied,
        Unknown,
        Pending,
    }

    #[derive(Clone)]
    struct MessageMoveProvider {
        account_id: AccountId,
        folders: Vec<MailFolder>,
        offline: OfflineProvider,
        response: MessageMoveActionResponse,
        calls: Arc<AtomicUsize>,
    }

    impl MailProvider for MessageMoveProvider {
        fn list_folders(
            &self,
            account_id: &AccountId,
        ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
            let matches_account = account_id == &self.account_id;
            let folders = self.folders.clone();
            async move {
                if matches_account {
                    Ok(folders)
                } else {
                    Err(MailOperationError::AccountMismatch)
                }
            }
        }

        fn list_messages(
            &self,
            folder: &FolderReference,
        ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_
        {
            self.offline.list_messages(folder)
        }

        fn message_delta_page(
            &self,
            folder: &FolderReference,
            _: MessageDeltaRequest,
        ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_
        {
            let matches_account = folder.account_id() == &self.account_id;
            async move {
                if !matches_account {
                    return Err(MailOperationError::AccountMismatch);
                }
                let progress = MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                    "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-move-view"
                        .to_owned(),
                ));
                Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                    MessageDeltaPage::new(Vec::new(), progress),
                    1_024,
                )))
            }
        }

        fn fetch_message_body(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
        ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
            self.offline.fetch_message_body(folder, message_id)
        }

        fn set_message_read_state(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
            read_state: ReadState,
        ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
            self.offline
                .set_message_read_state(folder, message_id, read_state)
        }

        fn set_message_follow_up_flag_status(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
            status: FollowUpFlagStatus,
        ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_
        {
            self.offline
                .set_message_follow_up_flag_status(folder, message_id, status)
        }

        fn move_message(
            &self,
            source_folder: &FolderReference,
            _: &MessageId,
            destination_folder: &FolderReference,
        ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
            let account_matches = source_folder.account_id() == destination_folder.account_id();
            let calls = Arc::clone(&self.calls);
            let response = self.response;
            async move {
                if !account_matches {
                    return Err(MailOperationError::AccountMismatch);
                }
                calls.fetch_add(1, Ordering::Relaxed);
                match response {
                    MessageMoveActionResponse::Confirm => Ok(()),
                    MessageMoveActionResponse::AccessDenied => {
                        Err(MailOperationError::AccessDenied)
                    }
                    MessageMoveActionResponse::Unknown => {
                        Err(MailOperationError::MoveOutcomeUnknown)
                    }
                    MessageMoveActionResponse::Pending => std::future::pending().await,
                }
            }
        }

        fn well_known_folder(
            &self,
            account_id: &AccountId,
            folder: WellKnownMailFolder,
        ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
            let folder_id = match folder {
                WellKnownMailFolder::Archive => "archive",
                WellKnownMailFolder::DeletedItems => "deleted-items",
            };
            if account_id == &self.account_id {
                ready(Ok(FolderReference::new(
                    account_id.clone(),
                    FolderId::try_from(folder_id.to_owned())
                        .expect("synthetic well-known folder ID is valid"),
                )))
            } else {
                ready(Err(MailOperationError::AccountMismatch))
            }
        }

        fn send_mail(
            &self,
            _: &AccountId,
            _: MailDraft,
        ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
            ready(Err(MailSendProviderError::ServiceUnavailable))
        }

        fn reply_to_message(
            &self,
            account_id: &AccountId,
            draft: MailReplyDraft,
        ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
            self.offline.reply_to_message(account_id, draft)
        }
    }

    struct SendGate {
        started: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    }

    #[derive(Clone)]
    struct PermissionProvider {
        offline: OfflineProvider,
        can_send: Arc<AtomicBool>,
        send_calls: Arc<AtomicUsize>,
        send_gate: Arc<Mutex<Option<SendGate>>>,
        reply_sources: Arc<Mutex<Vec<(String, String, String)>>>,
        reply_error: Arc<Mutex<Option<MailSendProviderError>>>,
    }

    impl PermissionProvider {
        fn hold_next_send(
            &self,
        ) -> (
            tokio::sync::oneshot::Receiver<()>,
            tokio::sync::oneshot::Sender<()>,
        ) {
            let (started, started_receiver) = tokio::sync::oneshot::channel();
            let (release, release_receiver) = tokio::sync::oneshot::channel();
            *self
                .send_gate
                .lock()
                .expect("fixture send-gate mutex is healthy") = Some(SendGate {
                started,
                release: release_receiver,
            });
            (started_receiver, release)
        }
    }

    impl MailProvider for PermissionProvider {
        fn list_folders(
            &self,
            account_id: &AccountId,
        ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
            self.offline.list_folders(account_id)
        }

        fn list_messages(
            &self,
            folder: &FolderReference,
        ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_
        {
            self.offline.list_messages(folder)
        }

        fn message_delta_page(
            &self,
            folder: &FolderReference,
            request: MessageDeltaRequest,
        ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_
        {
            self.offline.message_delta_page(folder, request)
        }

        fn fetch_message_body(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
        ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
            self.offline.fetch_message_body(folder, message_id)
        }

        fn set_message_read_state(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
            requested: ReadState,
        ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
            self.offline
                .set_message_read_state(folder, message_id, requested)
        }

        fn set_message_follow_up_flag_status(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
            status: FollowUpFlagStatus,
        ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_
        {
            self.offline
                .set_message_follow_up_flag_status(folder, message_id, status)
        }

        fn move_message(
            &self,
            source_folder: &FolderReference,
            message_id: &MessageId,
            destination_folder: &FolderReference,
        ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
            self.offline
                .move_message(source_folder, message_id, destination_folder)
        }

        fn well_known_folder(
            &self,
            account_id: &AccountId,
            folder: WellKnownMailFolder,
        ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
            self.offline.well_known_folder(account_id, folder)
        }

        fn send_mail(
            &self,
            account_id: &AccountId,
            _: MailDraft,
        ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
            let matches_account = account_id.as_str() == "synthetic-account";
            let can_send = Arc::clone(&self.can_send);
            let send_calls = Arc::clone(&self.send_calls);
            let send_gate = self
                .send_gate
                .lock()
                .expect("fixture send-gate mutex is healthy")
                .take();
            async move {
                if !matches_account {
                    return Err(MailSendProviderError::AccountMismatch);
                }
                send_calls.fetch_add(1, Ordering::Relaxed);
                if let Some(gate) = send_gate {
                    let _ = gate.started.send(());
                    let _ = gate.release.await;
                }
                if can_send.load(Ordering::Relaxed) {
                    Ok(())
                } else {
                    Err(MailSendProviderError::PermissionRequired)
                }
            }
        }

        fn reply_to_message(
            &self,
            account_id: &AccountId,
            draft: MailReplyDraft,
        ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
            let matches_account = account_id.as_str() == "synthetic-account"
                && draft.source().account_id() == account_id;
            let can_send = Arc::clone(&self.can_send);
            let send_calls = Arc::clone(&self.send_calls);
            let reply_sources = Arc::clone(&self.reply_sources);
            let reply_error = self
                .reply_error
                .lock()
                .expect("fixture reply-error mutex is healthy")
                .take();
            let source = draft.source();
            let account_id = account_id.as_str().to_owned();
            let folder_id = source.folder().folder_id().as_str().to_owned();
            let message_id = source.message_id().as_str().to_owned();
            let send_gate = self
                .send_gate
                .lock()
                .expect("fixture send-gate mutex is healthy")
                .take();
            async move {
                if !matches_account {
                    return Err(MailSendProviderError::AccountMismatch);
                }
                send_calls.fetch_add(1, Ordering::Relaxed);
                reply_sources
                    .lock()
                    .expect("fixture reply-source mutex is healthy")
                    .push((account_id, folder_id, message_id));
                if let Some(gate) = send_gate {
                    let _ = gate.started.send(());
                    let _ = gate.release.await;
                }
                if let Some(error) = reply_error {
                    Err(error)
                } else if can_send.load(Ordering::Relaxed) {
                    Ok(())
                } else {
                    Err(MailSendProviderError::PermissionRequired)
                }
            }
        }
    }

    struct PermissionGate {
        started: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    }

    struct FixturePermissionRequester {
        account_id: AccountId,
        requests: Arc<AtomicUsize>,
        gate: Mutex<Option<PermissionGate>>,
    }

    impl MailPermissionRequester for FixturePermissionRequester {
        fn request_send_permission<'a>(
            &'a self,
            account_id: &'a AccountId,
        ) -> Pin<Box<dyn Future<Output = Result<(), AccountOperationError>> + Send + 'a>> {
            let matches_account = account_id == &self.account_id;
            let requests = Arc::clone(&self.requests);
            let gate = self
                .gate
                .lock()
                .expect("fixture permission-gate mutex is healthy")
                .take();
            Box::pin(async move {
                if !matches_account {
                    return Err(AccountOperationError::AccountMismatch);
                }
                requests.fetch_add(1, Ordering::Relaxed);
                if let Some(gate) = gate {
                    let _ = gate.started.send(());
                    let _ = gate.release.await;
                }
                Ok(())
            })
        }
    }

    #[derive(Clone, Default)]
    struct FixtureCache(Arc<Mutex<FixtureData>>);

    #[derive(Default)]
    struct FixtureData {
        account_id: Option<AccountId>,
        folders: Vec<MailFolder>,
        messages: Vec<MailMessage>,
        body: Option<MessageBody>,
        sync_state: Option<MailSyncState>,
        read_state_write_error: Option<MailCacheError>,
        folder_write_sync_required: bool,
        pending_send: Option<PendingMailSend>,
        fail_next_pending_send_stage: bool,
        fail_next_pending_send_clear: bool,
        move_error: Option<MailCacheError>,
    }

    impl MailCache for FixtureCache {
        fn replace_folders(
            &self,
            account_id: &AccountId,
            folders: Vec<MailFolder>,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            let account_id = account_id.clone();
            async move {
                let mut data = self.0.lock().expect("fixture cache mutex is healthy");
                if data
                    .account_id
                    .as_ref()
                    .is_some_and(|saved| saved != &account_id)
                {
                    return Err(MailCacheError::AccountMismatch);
                }
                data.account_id = Some(account_id);
                data.folders = folders;
                Ok(())
            }
        }

        fn ensure_account_sync_scope(
            &self,
            _: &AccountId,
            _: SystemTime,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            ready(Ok(()))
        }

        fn ensure_sync_state(
            &self,
            _: &FolderReference,
            initial_cutoff: SystemTime,
        ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            ready(Ok(data
                .sync_state
                .get_or_insert_with(|| MailSyncState::initial(initial_cutoff))
                .clone()))
        }

        fn load_sync_state(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<Option<MailSyncState>, MailCacheError>> + Send + '_
        {
            ready(Ok(self
                .0
                .lock()
                .expect("fixture cache mutex is healthy")
                .sync_state
                .clone()))
        }

        fn commit_delta_page(
            &self,
            _: &FolderReference,
            expected_state: &MailSyncState,
            page: MessageDeltaPage,
            completed_at: SystemTime,
        ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
            let changes_empty = page.changes().is_empty();
            let progress = page.into_parts().1;
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            let Some(current) = data.sync_state.as_ref() else {
                return ready(Err(MailCacheError::InvalidData));
            };
            if current != expected_state {
                return ready(Err(MailCacheError::SyncStateChanged));
            }
            let MessageDeltaProgress::Complete(delta_link) = progress else {
                return ready(Err(MailCacheError::InvalidData));
            };
            if !changes_empty {
                return ready(Err(MailCacheError::InvalidData));
            }
            let Some(checkpoint_revision) = current.checkpoint_revision().checked_add(1) else {
                return ready(Err(MailCacheError::InvalidData));
            };
            let state = MailSyncState::from_checkpoint(
                current.cutoff(),
                Some(delta_link),
                None,
                Some(completed_at),
                current.full_sync_generation(),
                checkpoint_revision,
            );
            match state {
                Ok(state) => {
                    data.sync_state = Some(state.clone());
                    data.folder_write_sync_required = false;
                    ready(Ok(state))
                }
                Err(error) => ready(Err(error)),
            }
        }

        fn begin_full_resync(
            &self,
            _: &FolderReference,
            expected_state: &MailSyncState,
            reset_link: MessageDeltaLink,
        ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            let Some(current) = data.sync_state.as_ref() else {
                return ready(Err(MailCacheError::InvalidData));
            };
            if current != expected_state {
                return ready(Err(MailCacheError::SyncStateChanged));
            }
            let Some(generation) = current.full_sync_generation().unwrap_or(0).checked_add(1)
            else {
                return ready(Err(MailCacheError::InvalidData));
            };
            let Some(checkpoint_revision) = current.checkpoint_revision().checked_add(1) else {
                return ready(Err(MailCacheError::InvalidData));
            };
            let state = MailSyncState::from_checkpoint(
                current.cutoff(),
                current.delta_link().cloned(),
                Some(reset_link),
                current.last_synchronized_at(),
                Some(generation),
                checkpoint_revision,
            );
            match state {
                Ok(state) => {
                    data.sync_state = Some(state.clone());
                    ready(Ok(state))
                }
                Err(error) => ready(Err(error)),
            }
        }

        fn list_folders(
            &self,
            account_id: &AccountId,
        ) -> impl Future<Output = Result<Vec<MailFolder>, MailCacheError>> + Send + '_ {
            let account_id = account_id.clone();
            async move {
                let data = self.0.lock().expect("fixture cache mutex is healthy");
                if data
                    .account_id
                    .as_ref()
                    .is_some_and(|saved| saved != &account_id)
                {
                    return Err(MailCacheError::AccountMismatch);
                }
                Ok(data.folders.clone())
            }
        }

        fn list_messages(
            &self,
            folder: &FolderReference,
        ) -> impl Future<Output = Result<Vec<MailMessage>, MailCacheError>> + Send + '_ {
            let folder = folder.clone();
            let messages = self
                .0
                .lock()
                .expect("fixture cache mutex is healthy")
                .messages
                .iter()
                .filter(|message| message.folder() == &folder)
                .cloned()
                .collect();
            ready(Ok(messages))
        }

        fn read_message(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
        ) -> impl Future<
            Output = Result<Option<(MailMessage, Option<MessageBody>)>, MailCacheError>,
        > + Send
        + '_ {
            let message_id = message_id.clone();
            let folder = folder.clone();
            async move {
                let data = self.0.lock().expect("fixture cache mutex is healthy");
                Ok(data
                    .messages
                    .iter()
                    .find(|message| message.folder() == &folder && message.id() == &message_id)
                    .cloned()
                    .map(|message| (message, data.body.clone())))
            }
        }

        fn message_read_state(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
        ) -> impl Future<Output = Result<Option<ReadState>, MailCacheError>> + Send + '_ {
            let folder = folder.clone();
            let message_id = message_id.clone();
            async move {
                let data = self.0.lock().expect("fixture cache mutex is healthy");
                if data
                    .account_id
                    .as_ref()
                    .is_some_and(|account_id| account_id != folder.account_id())
                {
                    return Err(MailCacheError::AccountMismatch);
                }
                Ok(data
                    .messages
                    .iter()
                    .find(|message| message.folder() == &folder && message.id() == &message_id)
                    .map(|message| message.metadata().read_state()))
            }
        }

        fn set_message_read_state(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
            read_state: ReadState,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            let folder = folder.clone();
            let message_id = message_id.clone();
            async move {
                let mut data = self.0.lock().expect("fixture cache mutex is healthy");
                if let Some(error) = data.read_state_write_error {
                    return Err(error);
                }
                let Some(message) = data
                    .messages
                    .iter_mut()
                    .find(|message| message.folder() == &folder && message.id() == &message_id)
                else {
                    return Err(MailCacheError::MessageNotCached);
                };
                let mut patch = MessageMetadataPatch::new();
                patch.set_read_state(read_state);
                message.apply_patch(patch);
                data.folder_write_sync_required = false;
                Ok(())
            }
        }

        fn folder_write_sync_required(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<bool, MailCacheError>> + Send + '_ {
            ready(Ok(self
                .0
                .lock()
                .expect("fixture cache mutex is healthy")
                .folder_write_sync_required))
        }

        fn require_folder_write_sync(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            self.0
                .lock()
                .expect("fixture cache mutex is healthy")
                .folder_write_sync_required = true;
            ready(Ok(()))
        }

        fn clear_folder_write_sync_requirement(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            self.0
                .lock()
                .expect("fixture cache mutex is healthy")
                .folder_write_sync_required = false;
            ready(Ok(()))
        }

        fn set_message_follow_up_flag_status(
            &self,
            folder: &FolderReference,
            message_id: &MessageId,
            status: FollowUpFlagStatus,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            let folder = folder.clone();
            let message_id = message_id.clone();
            async move {
                let mut data = self.0.lock().expect("fixture cache mutex is healthy");
                if let Some(error) = data.read_state_write_error {
                    return Err(error);
                }
                let Some(message) = data
                    .messages
                    .iter_mut()
                    .find(|message| message.folder() == &folder && message.id() == &message_id)
                else {
                    return Err(MailCacheError::MessageNotCached);
                };
                let mut patch = MessageMetadataPatch::new();
                patch.set_follow_up_flag_status(Some(status));
                message.apply_patch(patch);
                data.folder_write_sync_required = false;
                Ok(())
            }
        }

        fn move_message(
            &self,
            source_folder: &FolderReference,
            destination_folder: &FolderReference,
            message_id: &MessageId,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            if let Some(error) = data.move_error {
                return ready(Err(error));
            }
            if data
                .account_id
                .as_ref()
                .is_some_and(|saved| saved != source_folder.account_id())
                || source_folder.account_id() != destination_folder.account_id()
            {
                return ready(Err(MailCacheError::AccountMismatch));
            }
            if source_folder == destination_folder
                || !data
                    .folders
                    .iter()
                    .any(|folder| folder.reference() == destination_folder)
            {
                return ready(Err(MailCacheError::InvalidData));
            }
            let Some(index) = data.messages.iter().position(|message| {
                message.folder() == source_folder && message.id() == message_id
            }) else {
                return ready(Err(MailCacheError::MessageNotCached));
            };
            let source = data.messages.remove(index);
            data.messages.push(MailMessage::new(
                destination_folder.clone(),
                source.id().clone(),
                source.metadata().clone(),
            ));
            ready(Ok(()))
        }

        fn store_message_body(
            &self,
            _: &FolderReference,
            _: &MessageId,
            body: MessageBody,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            self.0.lock().expect("fixture cache mutex is healthy").body = Some(body);
            ready(Ok(()))
        }

        fn load_pending_send(
            &self,
            _: &AccountId,
        ) -> impl Future<Output = Result<Option<PendingMailSend>, MailCacheError>> + Send + '_
        {
            ready(Ok(self
                .0
                .lock()
                .expect("fixture cache mutex is healthy")
                .pending_send
                .clone()))
        }

        fn stage_pending_send(
            &self,
            pending: &PendingMailSend,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            if data.fail_next_pending_send_stage {
                data.fail_next_pending_send_stage = false;
                return ready(Err(MailCacheError::Unavailable));
            }
            if data.pending_send.is_some() {
                return ready(Err(MailCacheError::SendAlreadyPending));
            }
            data.pending_send = Some(pending.clone());
            ready(Ok(()))
        }

        fn clear_pending_send(
            &self,
            _: &AccountId,
            operation_id: MailSendOperationId,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            if data.fail_next_pending_send_clear {
                data.fail_next_pending_send_clear = false;
                return ready(Err(MailCacheError::Unavailable));
            }
            match data
                .pending_send
                .as_ref()
                .map(PendingMailSend::operation_id)
            {
                Some(saved) if saved == operation_id => data.pending_send = None,
                Some(_) => return ready(Err(MailCacheError::PendingSendMismatch)),
                None => return ready(Err(MailCacheError::PendingSendNotFound)),
            }
            ready(Ok(()))
        }

        fn remove_account(
            &self,
            _: &AccountId,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            ready(Ok(()))
        }
    }

    fn fixture(profile_account: AccountId) -> (AccountProfile, FixtureCache, MessageId) {
        let folder_reference = FolderReference::new(
            profile_account.clone(),
            FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
        );
        let folder = MailFolder::new(folder_reference.clone(), None, "Inbox".to_owned())
            .expect("synthetic folder is valid");
        let archive_reference = FolderReference::new(
            profile_account.clone(),
            FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
        );
        let archive = MailFolder::new(archive_reference, None, "Archive".to_owned())
            .expect("synthetic archive folder is valid");
        let message_id =
            MessageId::try_from("message-1".to_owned()).expect("synthetic message ID is valid");
        let sender = Mailbox::try_new(
            "ada@example.test".to_owned(),
            Some("Ada Example".to_owned()),
        )
        .expect("synthetic mailbox is valid");
        let message = MailMessage::new(
            folder_reference,
            message_id.clone(),
            einfach_mail_domain::MessageMetadata::new(
                Some("Synthetic subject".to_owned()),
                einfach_mail_domain::MessageOrigin::new(Some(sender), None),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                einfach_mail_domain::ReadState::Unread,
                None,
            ),
        );
        let cache = FixtureCache::default();
        {
            let mut data = cache.0.lock().expect("fixture cache mutex is healthy");
            data.account_id = Some(profile_account.clone());
            data.folders.push(folder);
            data.folders.push(archive);
            data.messages.push(message);
            data.body = Some(MessageBody::new(
                MessageBodyType::Text,
                "Synthetic offline body".to_owned(),
            ));
            data.sync_state = Some(
                MailSyncState::from_checkpoint(
                    UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                    Some(MessageDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/delta?token=synthetic".to_owned(),
                    )),
                    None,
                    Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
                    None,
                    0,
                )
                .expect("synthetic sync checkpoint is valid"),
            );
        }
        let profile = AccountProfile::try_new(
            profile_account,
            Some("Ada Example".to_owned()),
            Some("ada@example.test".to_owned()),
            Some("ada@tenant.example.test".to_owned()),
        )
        .expect("synthetic account profile is valid");
        (profile, cache, message_id)
    }

    fn message_move_provider(
        cache: &FixtureCache,
        response: MessageMoveActionResponse,
        calls: Arc<AtomicUsize>,
    ) -> MessageMoveProvider {
        let cache_state = cache.0.lock().expect("fixture cache mutex is healthy");
        let account_id = cache_state
            .account_id
            .clone()
            .expect("fixture cache has an account");
        let mut folders = cache_state.folders.clone();
        folders.push(
            MailFolder::new(
                FolderReference::new(
                    account_id.clone(),
                    FolderId::try_from("deleted-items".to_owned())
                        .expect("synthetic Deleted Items ID is valid"),
                ),
                None,
                "Gelöschte Elemente".to_owned(),
            )
            .expect("synthetic Deleted Items folder is valid"),
        );
        MessageMoveProvider {
            account_id,
            folders,
            offline: OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            response,
            calls,
        }
    }

    fn draw_mail_view(context: &mut VisualTestContext) {
        context.update(|window, app| {
            let _ = window.draw(app);
        });
    }

    fn open_cached_message_and_click_read_state(context: &mut VisualTestContext) {
        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        let action = context
            .debug_bounds("message-read-state")
            .expect("opened message exposes its read-state action");
        context.simulate_click(action.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
    }

    fn open_cached_message_and_click_follow_up_flag(context: &mut VisualTestContext) {
        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        let action = context
            .debug_bounds("message-follow-up-flag")
            .expect("opened message exposes its follow-up flag action");
        context.simulate_click(action.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
    }

    fn click_composer_sheet_close(context: &mut VisualTestContext) {
        let composer = context
            .debug_bounds("mail-composer")
            .expect("composer exposes its test bounds");
        context.simulate_click(
            point(composer.right() - px(14.), composer.top() - px(16.)),
            Modifiers::default(),
        );
    }

    fn append_synthetic_messages(cache: &FixtureCache, message_count: usize) -> Vec<MessageId> {
        let mut state = cache.0.lock().expect("fixture cache mutex is healthy");
        let template = state
            .messages
            .first()
            .cloned()
            .expect("fixture cache has a synthetic message");
        let mut message_ids = vec![template.id().clone()];
        for index in 2..=message_count {
            let message_id = MessageId::try_from(format!("message-{index}"))
                .expect("synthetic message ID is valid");
            let template_metadata = template.metadata();
            let metadata = einfach_mail_domain::MessageMetadata::new(
                Some(format!("Synthetic subject {index}")),
                template_metadata.origin().clone(),
                *template_metadata.received_at(),
                template_metadata.read_state(),
                template_metadata.follow_up_flag_status(),
            );
            state.messages.push(MailMessage::new(
                template.folder().clone(),
                message_id.clone(),
                metadata,
            ));
            message_ids.push(message_id);
        }
        message_ids
    }

    fn press_window_key(context: &mut VisualTestContext, key: &str) {
        context.simulate_keystrokes(key);
    }

    const SYNTHETIC_MESSAGE_LABEL_PREFIX: &str =
        "Ada Example <ada@example.test>. Synthetic subject";

    fn assert_accessible_message_list(context: &mut VisualTestContext) {
        let (lists, options) = context.update(|window, _| {
            (
                window.find_all_by_role(gpui_kit::Role::ListBox),
                window.find_all_by_role(gpui_kit::Role::ListBoxOption),
            )
        });
        assert_eq!(lists.len(), 1, "mail has one message listbox");
        assert_eq!(lists[0].label(), Some("Messages"));
        assert!(!options.is_empty(), "visible messages are listbox options");
        assert!(
            options.iter().all(|option| option
                .label()
                .is_some_and(|label| label.starts_with(SYNTHETIC_MESSAGE_LABEL_PREFIX))),
            "every visible message option exposes its sender and subject"
        );
        assert!(
            options
                .iter()
                .all(|option| option.selected() == Some(false))
        );
    }

    fn accessible_message_list_focus(context: &mut VisualTestContext) -> Option<bool> {
        let lists = context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBox));
        assert_eq!(lists.len(), 1, "mail has one message listbox");
        lists[0].focused()
    }

    fn assert_selected_message_and_accessible_label(
        context: &mut VisualTestContext,
        view: &gpui_kit::Entity<MailView>,
        expected_message_id: Option<&MessageId>,
        expected_label: &str,
    ) {
        let selected = context
            .cx
            .update(|cx| view.read(cx).selected_message_id.clone());
        assert_eq!(selected.as_ref(), expected_message_id);

        let options =
            context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBoxOption));
        let selected_option_labels: Vec<_> = options
            .iter()
            .filter(|option| option.selected() == Some(true))
            .map(|option| option.label())
            .collect();
        assert_eq!(
            selected_option_labels,
            [Some(expected_label)],
            "exactly one message option is exposed as selected"
        );
    }

    #[gpui_kit::test]
    async fn cached_message_list_navigates_by_keyboard_and_keeps_one_tab_stop(
        cx: &mut TestAppContext,
    ) {
        const FIRST_MESSAGE_LABEL: &str = SYNTHETIC_MESSAGE_LABEL_PREFIX;
        const SECOND_MESSAGE_LABEL: &str = "Ada Example <ada@example.test>. Synthetic subject 2";
        const LAST_MESSAGE_LABEL: &str = "Ada Example <ada@example.test>. Synthetic subject 20";

        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_message_navigation_keys(cx);
        });
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let message_ids = append_synthetic_messages(&cache, 20);
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }

        assert_accessible_message_list(context);
        assert_eq!(accessible_message_list_focus(context), Some(false));

        context.update(gpui_kit::Window::focus_next);
        let mut list_focused = false;
        for _ in 0..20 {
            press_window_key(context, "tab");
            list_focused =
                context.update(|window, cx| view.read(cx).message_list_focus.is_focused(window));
            if list_focused {
                break;
            }
        }
        assert!(list_focused, "Tab reaches the cached message list");
        assert_eq!(accessible_message_list_focus(context), Some(true));

        press_window_key(context, "up");
        context.run_until_parked();
        draw_mail_view(context);
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.last(),
            LAST_MESSAGE_LABEL,
        );
        press_window_key(context, "down");
        context.run_until_parked();
        draw_mail_view(context);
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.last(),
            LAST_MESSAGE_LABEL,
        );

        press_window_key(context, "home");
        context.run_until_parked();
        draw_mail_view(context);
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.first(),
            FIRST_MESSAGE_LABEL,
        );
        press_window_key(context, "up");
        context.run_until_parked();
        draw_mail_view(context);
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.first(),
            FIRST_MESSAGE_LABEL,
        );
        press_window_key(context, "down");
        context.run_until_parked();
        draw_mail_view(context);
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.get(1),
            SECOND_MESSAGE_LABEL,
        );
        press_window_key(context, "end");
        context.run_until_parked();
        draw_mail_view(context);
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.last(),
            LAST_MESSAGE_LABEL,
        );
        assert!(
            context
                .cx
                .update(|cx| matches!(&view.read(cx).message_state, MessageState::Opened(_)))
        );
        let row = context
            .debug_bounds("message-row-message-20")
            .expect("End scrolls the selected message into view");
        let list_bounds = context.update(|window, _| window.find("cached-message-list").bounds());
        assert!(
            row.top() >= list_bounds.top() && row.bottom() <= list_bounds.bottom(),
            "End keeps the selected message inside the list viewport"
        );
        let scroll_offset = context
            .cx
            .update(|cx| view.read(cx).message_list_scroll.offset());
        assert!(scroll_offset.y < px(0.));
        context.cx.update(|cx| {
            view.update(cx, |view, _| {
                view.message_list_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
            });
        });
        context.run_until_parked();
        draw_mail_view(context);
        let first_row = context
            .debug_bounds("message-row-message-1")
            .expect("the first message is visible after scrolling to the top");
        context.simulate_click(first_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.first(),
            FIRST_MESSAGE_LABEL,
        );
        assert!(
            context.update(|window, cx| { view.read(cx).message_list_focus.is_focused(window) })
        );
        assert_eq!(accessible_message_list_focus(context), Some(true));

        press_window_key(context, "shift-tab");
        context.update(|window, cx| {
            assert!(!view.read(cx).message_list_focus.is_focused(window));
        });
        press_window_key(context, "up");
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.first(),
            FIRST_MESSAGE_LABEL,
        );
        press_window_key(context, "tab");
        assert!(
            context.update(|window, cx| { view.read(cx).message_list_focus.is_focused(window) })
        );
        assert_eq!(accessible_message_list_focus(context), Some(true));
        press_window_key(context, "tab");
        context.update(|window, cx| {
            assert!(!view.read(cx).message_list_focus.is_focused(window));
        });
        assert_eq!(accessible_message_list_focus(context), Some(false));
        press_window_key(context, "up");
        assert_selected_message_and_accessible_label(
            context,
            &view,
            message_ids.first(),
            FIRST_MESSAGE_LABEL,
        );
    }

    #[test]
    fn message_navigation_handles_empty_lists_and_stops_at_boundaries() {
        let cases = [
            (MessageNavigation::Previous, None, 0, None),
            (MessageNavigation::Next, None, 0, None),
            (MessageNavigation::First, None, 0, None),
            (MessageNavigation::Last, None, 0, None),
            (MessageNavigation::Previous, None, 3, Some(2)),
            (MessageNavigation::Next, None, 3, Some(0)),
            (MessageNavigation::Previous, Some(0), 3, Some(0)),
            (MessageNavigation::Next, Some(2), 3, Some(2)),
            (MessageNavigation::First, Some(1), 3, Some(0)),
            (MessageNavigation::Last, Some(1), 3, Some(2)),
        ];

        for (navigation, selected_index, message_count, expected_index) in cases {
            assert_eq!(
                navigation.target_index(selected_index, message_count),
                expected_index
            );
        }
    }

    #[gpui_kit::test]
    async fn message_move_menu_moves_cached_message_and_opens_destination(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let destination = cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .folders
            .iter()
            .find(|folder| folder.display_name() == "Archive")
            .expect("fixture cache contains a visible archive folder")
            .reference()
            .clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::Confirm,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-move").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let move_action = context
            .debug_bounds("message-move")
            .expect("opened message exposes its native move menu");
        assert!(move_action.size.width > px(0.));
        context.cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply_message_move_menu_action(
                    MessageMoveMenuAction {
                        selected_message_id: message_id.clone(),
                        destination_folder: destination.clone(),
                        destination_label: "Archive".to_owned(),
                    },
                    cx,
                );
            });
        });

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.cx.update(|cx| {
                matches!(
                    view.read(cx).message_state,
                    MessageState::MoveCompleted { .. }
                )
            }) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("message-move-completed").is_some());
        assert!(context.cx.update(|cx| view.read(cx).messages.is_empty()));
        let archived_messages = cache
            .list_messages(&destination)
            .await
            .expect("moved message list is readable");
        assert_eq!(archived_messages.len(), 1);
        assert_eq!(archived_messages[0].id(), &message_id);

        let open_folder = context
            .debug_bounds("open-moved-message-folder")
            .expect("move confirmation offers the destination folder");
        context.simulate_click(open_folder.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.cx.update(|cx| {
                let view = view.read(cx);
                view.selected_folder.as_ref() == Some(&destination)
                    && view
                        .messages
                        .iter()
                        .any(|message| message.id() == &message_id)
            }) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert!(context.cx.update(|cx| {
            let view = view.read(cx);
            view.selected_folder.as_ref() == Some(&destination)
                && view
                    .messages
                    .iter()
                    .any(|message| message.id() == &message_id)
        }));
    }

    #[gpui_kit::test]
    async fn message_delete_action_moves_cached_message_to_deleted_items(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::Confirm,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-delete").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let delete_action = context
            .debug_bounds("message-delete")
            .expect("opened message exposes its native Delete action");
        context.simulate_click(delete_action.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-delete-completed").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let deleted_reference = FolderReference::new(
            account,
            FolderId::try_from("deleted-items".to_owned())
                .expect("synthetic Deleted Items ID is valid"),
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("message-delete-completed").is_some());
        assert!(context.cx.update(|cx| view.read(cx).messages.is_empty()));
        let deleted_messages = cache
            .list_messages(&deleted_reference)
            .await
            .expect("Deleted Items cache is readable");
        assert_eq!(deleted_messages.len(), 1);
        assert_eq!(deleted_messages[0].id(), &message_id);
        let deleted_message = cache
            .read_message(&deleted_reference, &message_id)
            .await
            .expect("Deleted Items message is readable")
            .expect("message body remains cached after deletion");
        assert_eq!(
            deleted_message
                .1
                .expect("cached body moves with its message")
                .content(),
            "Synthetic offline body"
        );
    }

    #[gpui_kit::test]
    async fn denied_message_delete_shows_permission_recovery_and_keeps_source(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::AccessDenied,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");
        let permission_review_requested = Arc::new(AtomicBool::new(false));
        let observed_permission_review = Arc::clone(&permission_review_requested);
        let _subscription = context.cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &MailViewEvent, _| {
                if matches!(event, MailViewEvent::ReviewMailWritePermission) {
                    observed_permission_review.store(true, Ordering::Relaxed);
                }
            })
        });

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-delete").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let delete_action = context
            .debug_bounds("message-delete")
            .expect("opened message exposes its native Delete action");
        context.simulate_click(delete_action.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-delete-error").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("message-delete-error").is_some());
        assert!(
            context
                .debug_bounds("review-permissions-from-delete")
                .is_some()
        );
        let source_folder = FolderReference::new(
            account,
            FolderId::try_from("inbox".to_owned()).expect("synthetic inbox ID is valid"),
        );
        assert!(
            cache
                .read_message(&source_folder, &message_id)
                .await
                .expect("source cache lookup succeeds")
                .is_some()
        );
        let permission_review = context
            .debug_bounds("review-permissions-from-delete")
            .expect("denied Delete offers delegated permission review");
        context.simulate_click(permission_review.center(), Modifiers::default());
        context.run_until_parked();
        assert!(permission_review_requested.load(Ordering::Relaxed));
        assert!(context.cx.update(|cx| {
            matches!(
                &view.read(cx).well_known_folder_action_status,
                WellKnownFolderActionStatus::Failed {
                    destination: WellKnownMailFolder::DeletedItems,
                    error,
                    ..
                } if *error == MailWorkflowError::Provider(MailOperationError::AccessDenied)
            ) && view.read(cx).move_recovery_reason.is_none()
        }));
    }

    #[gpui_kit::test]
    async fn native_archive_action_moves_cached_message_to_archive(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::Confirm,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-archive").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let archive_action = context
            .debug_bounds("message-archive")
            .expect("opened message exposes its native Archive action");
        context.simulate_click(archive_action.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-move-completed").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let archive_reference = FolderReference::new(
            account,
            FolderId::try_from("archive".to_owned()).expect("synthetic Archive ID is valid"),
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("message-move-completed").is_some());
        assert!(context.cx.update(|cx| view.read(cx).messages.is_empty()));
        let archived_message = cache
            .read_message(&archive_reference, &message_id)
            .await
            .expect("Archive cache is readable")
            .expect("message body remains cached after archiving");
        assert_eq!(
            archived_message
                .1
                .expect("cached body moves with the message")
                .content(),
            "Synthetic offline body"
        );
        let open_archive = context
            .debug_bounds("open-moved-message-folder")
            .expect("archive confirmation offers to open Archive");
        context.simulate_click(open_archive.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.cx.update(|cx| {
                let view = view.read(cx);
                view.selected_folder.as_ref() == Some(&archive_reference)
                    && view
                        .messages
                        .iter()
                        .any(|message| message.id() == &message_id)
            }) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert!(context.cx.update(|cx| {
            let view = view.read(cx);
            view.selected_folder.as_ref() == Some(&archive_reference)
                && view
                    .messages
                    .iter()
                    .any(|message| message.id() == &message_id)
        }));
    }
    #[gpui_kit::test]
    async fn pending_message_delete_keeps_the_native_action_busy(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::Pending,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-delete").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let delete_action = context
            .debug_bounds("message-delete")
            .expect("opened message exposes its native Delete action");
        context.simulate_click(delete_action.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-delete-pending").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("message-delete-pending").is_some());
        assert!(context.cx.update(|cx| {
            view.read(cx).well_known_folder_action_in_progress_for(
                &message_id,
                WellKnownMailFolder::DeletedItems,
            )
        }));
    }

    #[gpui_kit::test]
    async fn unknown_message_delete_requires_sync_and_keeps_source_cached(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::Unknown,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-delete").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let delete_action = context
            .debug_bounds("message-delete")
            .expect("opened message exposes its native Delete action");
        context.simulate_click(delete_action.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-move-recovery").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("message-move-recovery").is_some());
        assert!(context.debug_bounds("sync-after-message-move").is_some());
        assert!(context.cx.update(|cx| {
            let view = view.read(cx);
            matches!(
                view.move_recovery_reason,
                Some(MoveRecoveryReason::OutcomeUnknown)
            ) && matches!(view.message_state, MessageState::Opened(_))
                && !view.message_move_allowed()
        }));
        let source = FolderReference::new(
            account,
            FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
        );
        assert!(
            cache
                .read_message(&source, &message_id)
                .await
                .expect("source cache lookup succeeds after unknown outcome")
                .is_some()
        );
    }

    #[gpui_kit::test]
    async fn folder_write_sync_requirement_disables_and_blocks_message_move(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account.clone());
        let destination = cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .folders
            .iter()
            .find(|folder| folder.display_name() == "Archive")
            .expect("fixture cache contains a visible archive folder")
            .reference()
            .clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::Confirm,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-move").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }

        context.cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.selected_folder_write_sync_required = true;
                cx.notify();
            });
        });
        draw_mail_view(context);
        let move_action = context
            .debug_bounds("message-move")
            .expect("opened message still exposes its disabled move action");
        context.simulate_click(move_action.center(), Modifiers::default());
        context.run_until_parked();
        assert!(context.debug_bounds("read-state-reconciliation").is_some());
        assert!(context.cx.update(|cx| {
            let view = view.read(cx);
            !view.message_move_allowed()
                && view
                    .prepare_message_move(destination.clone(), "Archive".to_owned())
                    .is_none()
        }));
        context.cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply_message_move_menu_action(
                    MessageMoveMenuAction {
                        selected_message_id: message_id,
                        destination_folder: destination,
                        destination_label: "Archive".to_owned(),
                    },
                    cx,
                );
            });
        });

        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[gpui_kit::test]
    async fn unknown_message_move_shows_sync_recovery_and_keeps_source_cached(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account.clone());
        let destination = cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .folders
            .iter()
            .find(|folder| folder.display_name() == "Archive")
            .expect("fixture cache contains a visible archive folder")
            .reference()
            .clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::Unknown,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-move").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        context.cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.move_selected_message_to(destination.clone(), "Archive".to_owned(), cx);
            });
        });

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-move-recovery").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("message-move-recovery").is_some());
        assert!(context.debug_bounds("sync-after-message-move").is_some());
        assert!(context.cx.update(|cx| {
            let view = view.read(cx);
            view.move_recovery_reason.is_some()
                && matches!(view.message_state, MessageState::Opened(_))
        }));
        let source = FolderReference::new(
            account,
            FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
        );
        assert!(
            cache
                .read_message(&source, &message_id)
                .await
                .expect("source cache lookup succeeds")
                .is_some()
        );
    }

    #[gpui_kit::test]
    async fn confirmed_message_move_with_cache_failure_requests_sync(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account.clone());
        let destination = cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .folders
            .iter()
            .find(|folder| folder.display_name() == "Archive")
            .expect("fixture cache contains a visible archive folder")
            .reference()
            .clone();
        cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .move_error = Some(MailCacheError::Unavailable);
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = message_move_provider(
            &cache,
            MessageMoveActionResponse::Confirm,
            Arc::clone(&calls),
        );
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("source message is visible in the cached inbox");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-move").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        context.cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.move_selected_message_to(destination.clone(), "Archive".to_owned(), cx);
            });
        });

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-move-recovery").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("message-move-recovery").is_some());
        assert!(context.cx.update(|cx| {
            let view = view.read(cx);
            matches!(view.message_state, MessageState::Empty)
                && matches!(
                    view.move_recovery_reason,
                    Some(MoveRecoveryReason::CacheNeedsSync)
                )
        }));
        let source = FolderReference::new(
            account,
            FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
        );
        assert!(
            cache
                .read_message(&source, &message_id)
                .await
                .expect("source cache lookup succeeds")
                .is_some()
        );
    }

    #[gpui_kit::test]
    fn offline_mail_view_opens_cached_message_without_provider_body_request(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let provider = OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        };
        let body_fetches = Arc::clone(&provider.body_fetches);
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let row_bounds = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(row_bounds.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let opened_offline = context.cx.update(|cx| {
            matches!(
                &view.read(cx).message_state,
                MessageState::Opened(opened)
                    if opened.message().id() == &message_id
                        && opened.loaded_from_cache()
                        && matches!(
                            opened.body(),
                            MessageBodyForDisplay::PlainText(text)
                                if text.as_ref() == "Synthetic offline body"
                        )
            )
        });
        assert!(opened_offline);
        assert!(context.cx.update(|cx| {
            matches!(
                view.read(cx).move_recovery_reason,
                Some(MoveRecoveryReason::InitialSyncRequired)
            )
        }));
        assert_eq!(body_fetches.load(Ordering::Relaxed), 0);
    }

    #[gpui_kit::test]
    async fn confirmed_read_state_updates_the_view_and_encrypted_cache(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let folder = FolderReference::new(
            account.clone(),
            FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
        );
        let (profile, cache, message_id) = fixture(account);
        let updates = Arc::new(Mutex::new(Vec::new()));
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(ReadStateActionResponse::Confirm, Arc::clone(&updates)),
            cache.clone(),
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        open_cached_message_and_click_read_state(context);

        assert_eq!(
            context.cx.update(|cx| {
                view.read(cx)
                    .messages
                    .iter()
                    .find(|message| message.id() == &message_id)
                    .map(|message| message.metadata().read_state())
            }),
            Some(ReadState::Read)
        );
        assert!(context.cx.update(|cx| matches!(
            &view.read(cx).read_state_update,
            ReadStateUpdateStatus::Confirmed {
                state: ReadState::Read,
                ..
            }
        )));
        assert_eq!(
            updates
                .lock()
                .expect("fixture read-state log mutex is healthy")
                .as_slice(),
            [(message_id.clone(), ReadState::Read)]
        );
        assert_eq!(
            cache
                .message_read_state(&folder, &message_id)
                .await
                .expect("confirmed state is readable from the cache"),
            Some(ReadState::Read)
        );
    }

    #[gpui_kit::test]
    async fn confirmed_follow_up_flag_updates_the_view_and_encrypted_cache(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let folder = FolderReference::new(
            account.clone(),
            FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
        );
        let (profile, cache, message_id) = fixture(account);
        let updates = Arc::new(Mutex::new(Vec::new()));
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(
                ReadStateActionResponse::Confirm,
                Arc::new(Mutex::new(Vec::new())),
            )
            .with_flag_updates(Arc::clone(&updates)),
            cache.clone(),
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        open_cached_message_and_click_follow_up_flag(context);

        assert_eq!(
            context.cx.update(|cx| {
                view.read(cx)
                    .messages
                    .iter()
                    .find(|message| message.id() == &message_id)
                    .and_then(|message| message.metadata().follow_up_flag_status())
            }),
            Some(FollowUpFlagStatus::Flagged)
        );
        assert!(context.cx.update(|cx| matches!(
            &view.read(cx).follow_up_flag_update,
            FollowUpFlagUpdateStatus::Confirmed {
                status: FollowUpFlagStatus::Flagged,
                ..
            }
        )));
        assert_eq!(
            cache
                .read_message(&folder, &message_id)
                .await
                .expect("confirmed flag remains readable from the cache")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::Flagged)
        );
        assert_eq!(
            updates
                .lock()
                .expect("fixture flag-update log mutex is healthy")
                .as_slice(),
            [(message_id, FollowUpFlagStatus::Flagged)]
        );
        assert!(
            context
                .debug_bounds("message-follow-up-flag-feedback")
                .is_some()
        );
    }

    #[gpui_kit::test]
    fn uncertain_follow_up_flag_blocks_a_second_write(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let updates = Arc::new(Mutex::new(Vec::new()));
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(
                ReadStateActionResponse::Unconfirmed,
                Arc::new(Mutex::new(Vec::new())),
            )
            .with_flag_updates(Arc::clone(&updates)),
            cache.clone(),
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        open_cached_message_and_click_follow_up_flag(context);

        assert!(context.cx.update(|cx| matches!(
            &view.read(cx).follow_up_flag_update,
            FollowUpFlagUpdateStatus::Failed {
                error: MailWorkflowError::Provider(MailOperationError::MessageUpdateOutcomeUnknown),
                ..
            }
        )));
        assert!(
            context
                .cx
                .update(|cx| { view.read(cx).selected_follow_up_flag_request().is_none() })
        );
        assert!(
            context
                .debug_bounds("message-follow-up-flag-feedback")
                .is_some()
        );
        assert!(context.debug_bounds("read-state-reconciliation").is_some());

        let action_bounds = context
            .debug_bounds("message-follow-up-flag")
            .expect("follow-up action remains visible");
        context.simulate_click(action_bounds.center(), Modifiers::default());
        context.run_until_parked();
        assert_eq!(
            updates
                .lock()
                .expect("fixture flag-update log mutex is healthy")
                .as_slice(),
            [(message_id, FollowUpFlagStatus::Flagged)]
        );
        assert!(
            cache
                .0
                .lock()
                .expect("fixture cache mutex is healthy")
                .folder_write_sync_required
        );
    }

    #[gpui_kit::test]
    fn failed_initial_mail_sync_keeps_read_state_changes_disabled(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let updates = Arc::new(Mutex::new(Vec::new()));
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(ReadStateActionResponse::Confirm, Arc::clone(&updates))
                .with_sync_failure(),
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let row_bounds = context
            .debug_bounds("message-row-message-1")
            .expect("cached message remains visible after sync failure");
        context.simulate_click(row_bounds.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        let action_bounds = context
            .debug_bounds("message-read-state")
            .expect("read-state action remains visible after sync failure");
        context.simulate_click(action_bounds.center(), Modifiers::default());
        context.run_until_parked();

        assert!(context.cx.update(|cx| {
            let view = view.read(cx);
            view.selected_read_state_request().is_none()
                && matches!(&view.read_state_update, ReadStateUpdateStatus::Idle)
        }));
        assert!(context.debug_bounds("read-state-reconciliation").is_some());
        assert!(
            updates
                .lock()
                .expect("fixture read-state log mutex is healthy")
                .is_empty()
        );
        let state = context.cx.update(|cx| {
            view.read(cx)
                .messages
                .iter()
                .find(|message| message.id() == &message_id)
                .map(|message| message.metadata().read_state())
        });
        assert_eq!(state, Some(ReadState::Unread));
    }

    #[gpui_kit::test]
    async fn confirmed_read_state_stays_visible_when_cache_write_fails(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let folder = FolderReference::new(
            account.clone(),
            FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
        );
        let (profile, cache, message_id) = fixture(account);
        cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .read_state_write_error = Some(MailCacheError::Unavailable);
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(
                ReadStateActionResponse::Confirm,
                Arc::new(Mutex::new(Vec::new())),
            ),
            cache.clone(),
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        open_cached_message_and_click_read_state(context);

        let (visible_state, needs_sync) = context.cx.update(|cx| {
            let view = view.read(cx);
            let visible_state = view
                .messages
                .iter()
                .find(|message| message.id() == &message_id)
                .map(|message| message.metadata().read_state());
            let needs_sync = matches!(
                &view.read_state_update,
                ReadStateUpdateStatus::ConfirmedNeedsSync {
                    state: ReadState::Read,
                    ..
                }
            );
            (visible_state, needs_sync)
        });
        assert_eq!(visible_state, Some(ReadState::Read));
        assert!(needs_sync);
        assert_eq!(
            cache
                .message_read_state(&folder, &message_id)
                .await
                .expect("cache remains readable after a failed write"),
            Some(ReadState::Unread)
        );
        assert!(
            context
                .debug_bounds("message-read-state-feedback")
                .is_some()
        );
    }

    #[gpui_kit::test]
    fn uncertain_read_state_shows_sync_guidance_and_blocks_another_write(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let updates = Arc::new(Mutex::new(Vec::new()));
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(ReadStateActionResponse::Unconfirmed, Arc::clone(&updates)),
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        open_cached_message_and_click_read_state(context);

        let (state, request_blocked) = context.cx.update(|cx| {
            let view = view.read(cx);
            let state = view
                .messages
                .iter()
                .find(|message| message.id() == &message_id)
                .map(|message| message.metadata().read_state());
            let request_blocked = view.selected_read_state_request().is_none();
            (state, request_blocked)
        });
        assert_eq!(state, Some(ReadState::Unread));
        assert!(request_blocked);
        assert!(
            context
                .debug_bounds("message-read-state-feedback")
                .is_some()
        );
        assert!(context.debug_bounds("read-state-reconciliation").is_some());

        let action_bounds = context
            .debug_bounds("message-read-state")
            .expect("read-state action remains visible");
        context.simulate_click(action_bounds.center(), Modifiers::default());
        context.run_until_parked();
        assert_eq!(
            updates
                .lock()
                .expect("fixture read-state log mutex is healthy")
                .as_slice(),
            [(message_id, ReadState::Read)]
        );
    }

    #[gpui_kit::test]
    fn pending_read_state_keeps_account_navigation_open(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(
                ReadStateActionResponse::Pending,
                Arc::new(Mutex::new(Vec::new())),
            ),
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));
        let navigation_requested = Arc::new(AtomicBool::new(false));
        let observed_navigation = Arc::clone(&navigation_requested);
        let _subscription = context.cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &MailViewEvent, _| {
                if matches!(event, MailViewEvent::BackToAccount) {
                    observed_navigation.store(true, Ordering::Relaxed);
                }
            })
        });

        context.run_until_parked();
        draw_mail_view(context);
        let row_bounds = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(row_bounds.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        let action_bounds = context
            .debug_bounds("message-read-state")
            .expect("opened message exposes its read-state action");
        context.simulate_click(action_bounds.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        assert!(context.cx.update(|cx| matches!(
            &view.read(cx).read_state_update,
            ReadStateUpdateStatus::Pending {
                requested: ReadState::Read,
                ..
            }
        )));

        let account_bounds = context
            .debug_bounds("mail-account-navigation")
            .expect("account navigation remains visible while the write is pending");
        context.simulate_click(account_bounds.center(), Modifiers::default());
        context.run_until_parked();
        assert!(!navigation_requested.load(Ordering::Relaxed));
    }

    #[gpui_kit::test]
    fn denied_read_state_offers_account_permission_review(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(
                ReadStateActionResponse::Deny,
                Arc::new(Mutex::new(Vec::new())),
            ),
            cache.clone(),
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));
        let permission_review_requested = Arc::new(AtomicBool::new(false));
        let observed_review = Arc::clone(&permission_review_requested);
        let _subscription = context.cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &MailViewEvent, _| {
                if matches!(event, MailViewEvent::ReviewMailWritePermission) {
                    observed_review.store(true, Ordering::Relaxed);
                }
            })
        });

        open_cached_message_and_click_read_state(context);

        let review_bounds = context
            .debug_bounds("review-mail-permissions-from-message")
            .expect("denied write offers permission review");
        context.simulate_click(review_bounds.center(), Modifiers::default());
        context.run_until_parked();
        assert!(permission_review_requested.load(Ordering::Relaxed));
        assert_eq!(
            cache
                .0
                .lock()
                .expect("fixture cache mutex is healthy")
                .messages
                .iter()
                .find(|message| message.id() == &message_id)
                .map(|message| message.metadata().read_state()),
            Some(ReadState::Unread)
        );
    }

    #[gpui_kit::test]
    fn offline_read_state_shows_retry_guidance(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let workflow = Arc::new(MailWorkflow::new(
            ReadStateProvider::new(
                ReadStateActionResponse::Offline,
                Arc::new(Mutex::new(Vec::new())),
            ),
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        open_cached_message_and_click_read_state(context);

        assert!(context.cx.update(|cx| matches!(
            &view.read(cx).read_state_update,
            ReadStateUpdateStatus::Failed {
                error: MailWorkflowError::Provider(MailOperationError::ServiceUnavailable),
                ..
            }
        )));
        assert!(
            context
                .debug_bounds("message-read-state-feedback")
                .is_some()
        );
    }

    #[test]
    fn offline_read_state_failure_explains_how_to_retry() {
        assert_eq!(
            read_state_failure_message(
                ReadState::Read,
                MailWorkflowError::Provider(MailOperationError::ServiceUnavailable),
            ),
            "Couldn’t mark the message read. Check your connection and try again."
        );
    }

    #[test]
    fn uncertain_follow_up_flag_failure_explains_sync_recovery() {
        assert_eq!(
            follow_up_flag_failure_message(
                FollowUpFlagStatus::Flagged,
                MailWorkflowError::Provider(MailOperationError::MessageUpdateOutcomeUnknown),
            ),
            "Microsoft may have changed this message's follow-up flag, but Einfach Mail could not confirm it. Sync this folder before trying again."
        );
    }

    #[gpui_kit::test]
    fn cached_html_mail_view_sanitizes_content_and_opens_links_on_click(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
            MessageBodyType::Html,
            concat!(
                "<p><a href=\"https://mail.example.test/thread\">Open safe link</a></p>",
                "<p>Readable synthetic message content.</p>",
                "<img src=\"https://tracker.example.test/pixel\">",
                "<script>hidden script marker</script>"
            )
            .to_owned(),
        ));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let safe_html = context.cx.update(|cx| {
            let state = &view.read(cx).message_state;
            let MessageState::Opened(opened) = state else {
                return false;
            };
            let MessageBodyForDisplay::SanitizedHtml(html) = opened.body() else {
                return false;
            };
            html.as_str().contains("Open safe link")
                && !html.as_str().contains("tracker.example.test")
                && !html.as_str().contains("hidden script marker")
        });
        assert!(safe_html);

        let body_bounds = context
            .debug_bounds("mail-message-html-body")
            .expect("sanitized HTML body has a layout");
        let link_position = point(body_bounds.left() + px(36.), body_bounds.top() + px(36.));
        context.simulate_mouse_down(link_position, MouseButton::Right, Modifiers::default());
        context.simulate_mouse_up(link_position, MouseButton::Right, Modifiers::default());
        assert_eq!(context.opened_url(), None);
        context.simulate_click(link_position, Modifiers::default());
        assert_eq!(
            context.opened_url(),
            Some("https://mail.example.test/thread".to_owned())
        );
    }

    #[gpui_kit::test]
    fn cached_html_mail_view_does_not_open_unsafe_links(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
            MessageBodyType::Html,
            concat!(
                "<p><a href=\"javascript:alert(1)\">Unsafe synthetic link</a></p>",
                "<img src=\"https://tracker.example.test/pixel\">",
                "<script>hidden script marker</script>"
            )
            .to_owned(),
        ));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let sanitized_body = context.cx.update(|cx| {
            let MessageState::Opened(opened) = &view.read(cx).message_state else {
                return false;
            };
            let MessageBodyForDisplay::SanitizedHtml(html) = opened.body() else {
                return false;
            };
            !html.as_str().contains("href=")
                && !html.as_str().contains("tracker.example.test")
                && !html.as_str().contains("hidden script marker")
                && html.as_str().contains("Unsafe synthetic link")
        });
        assert!(sanitized_body);

        let body_bounds = context
            .debug_bounds("mail-message-html-body")
            .expect("sanitized HTML body has a layout");
        context.simulate_click(
            point(body_bounds.left() + px(12.), body_bounds.top() + px(12.)),
            Modifiers::default(),
        );
        assert_eq!(context.opened_url(), None);
    }

    #[gpui_kit::test]
    fn cached_html_mail_view_supports_selection_and_copy(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
            MessageBodyType::Html,
            "<p>Readable synthetic content for selection and copy.</p>".to_owned(),
        ));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (_, context) = cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let body_bounds = context
            .debug_bounds("mail-message-html-body")
            .expect("sanitized HTML body has a layout");
        let start = point(body_bounds.left() + px(25.), body_bounds.top() + px(34.));
        let end = point(body_bounds.left() + px(285.), body_bounds.top() + px(34.));
        context.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        draw_mail_view(context);
        context.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        draw_mail_view(context);
        context.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        draw_mail_view(context);

        let selected_text = context.update(TextSelection::selected_text);
        assert!(selected_text.contains("Readable synthetic content"));
        context.simulate_keystrokes("ctrl-c");
        let copied_text = context.update(|_, app| {
            app.read_from_clipboard()
                .and_then(|clipboard| clipboard.text())
        });
        assert_eq!(copied_text, Some(selected_text.trim_end().to_owned()));
    }

    #[gpui_kit::test]
    fn oversized_sanitized_html_shows_selectable_noninteractive_text(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let body_content = format!(
            "<p>Readable fallback content. <a href=\"https://mail.example.test/path\">link destination</a> {}</p>",
            ">".repeat(8 * 1024 * 1024 / 4)
        );
        cache.0.lock().expect("fixture cache mutex is healthy").body =
            Some(MessageBody::new(MessageBodyType::Html, body_content));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let status = context
            .debug_bounds("mail-message-html-fallback-status")
            .expect("plain-text fallback explains why formatting was removed");
        assert!(status.size.width > px(0.));
        let body_bounds = context
            .debug_bounds("mail-message-html-fallback-body")
            .expect("plain-text fallback has a selectable content region");

        let contains_fallback_text = context.cx.update(|cx| {
            matches!(
                &view.read(cx).message_state,
                MessageState::Opened(opened)
                    if matches!(
                        opened.body(),
                        MessageBodyForDisplay::PlainTextFallback(content)
                            if content.as_str().contains("Readable&#32;fallback&#32;content")
                                && content.as_str().contains("link&#32;destination")
                    )
            )
        });
        assert!(contains_fallback_text);

        context.simulate_click(
            point(body_bounds.left() + px(280.), body_bounds.top() + px(10.)),
            Modifiers::default(),
        );
        assert_eq!(context.opened_url(), None);
        context.simulate_keystrokes("ctrl-a");

        let selected_text = context.update(TextSelection::selected_text);
        assert!(selected_text.contains("Readable fallback content"));
        context.simulate_keystrokes("ctrl-c");
        let copied_text = context.update(|_, app| {
            app.read_from_clipboard()
                .and_then(|clipboard| clipboard.text())
        });
        assert_eq!(copied_text, Some(selected_text.trim_end().to_owned()));
    }

    #[gpui_kit::test]
    fn oversized_cached_html_shows_a_safe_reader_fallback(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
            MessageBodyType::Html,
            "x".repeat(8 * 1024 * 1024 + 1),
        ));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let is_over_limit = context.cx.update(|cx| {
            matches!(
                view.read(cx).message_state,
                MessageState::Opened(ref opened)
                    if opened.body() == &MessageBodyForDisplay::HtmlTooLarge
            )
        });
        assert!(is_over_limit);
        assert!(
            context
                .debug_bounds("mail-message-body-too-large")
                .is_some()
        );
    }

    #[gpui_kit::test]
    fn cache_snapshot_failure_survives_a_successful_sync(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));
        context.run_until_parked();

        context.cx.update(|cx| {
            view.update(cx, |view, _| {
                view.finish_sync(
                    Ok(()),
                    Err(MailWorkflowError::Cache(MailCacheError::Unavailable)),
                );
            });
        });
        let preserved_error = context.cx.update(|cx| {
            matches!(
                &view.read(cx).sync_status,
                SyncStatus::Failed(MailWorkflowError::Cache(MailCacheError::Unavailable))
            )
        });

        assert!(preserved_error);
    }

    #[gpui_kit::test]
    fn reset_progress_is_announced_in_the_mail_status(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));
        context.run_until_parked();

        context.cx.update(|cx| {
            view.update(cx, |view, _| {
                view.apply_sync_progress(MailSyncProgress::Rebuilding);
            });
        });
        let status = context.cx.update(|cx| view.read(cx).sync_status_label());

        assert!(status.starts_with("Rebuilding mail"));
    }

    #[gpui_kit::test]
    fn failed_rebuild_status_explains_how_to_resume(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        {
            let mut data = cache.0.lock().expect("fixture cache mutex is healthy");
            let current = data.sync_state.as_ref().expect("fixture sync state exists");
            data.sync_state = Some(
                MailSyncState::from_checkpoint(
                    current.cutoff(),
                    current.delta_link().cloned(),
                    Some(MessageDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/delta?token=synthetic-reset".to_owned(),
                    )),
                    current.last_synchronized_at(),
                    Some(1),
                    1,
                )
                .expect("synthetic rebuild checkpoint is valid"),
            );
        }
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));
        context.run_until_parked();

        let status = context.cx.update(|cx| view.read(cx).sync_status_label());

        assert!(status.contains("saved mail rebuild is incomplete"));
        assert!(status.contains("Choose Sync to resume it."));
    }

    #[gpui_kit::test]
    async fn reply_composer_uses_the_cached_source_and_requires_an_explicit_send(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_mail_composer_keys(cx);
        });
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account.clone());
        let can_send = Arc::new(AtomicBool::new(false));
        let send_calls = Arc::new(AtomicUsize::new(0));
        let reply_sources = Arc::new(Mutex::new(Vec::new()));
        let permission_requests = Arc::new(AtomicUsize::new(0));
        let provider = PermissionProvider {
            offline: OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            can_send: Arc::clone(&can_send),
            send_calls: Arc::clone(&send_calls),
            send_gate: Arc::default(),
            reply_sources: Arc::clone(&reply_sources),
            reply_error: Arc::default(),
        };
        let requester: Arc<dyn MailPermissionRequester> = Arc::new(FixturePermissionRequester {
            account_id: account,
            requests: Arc::clone(&permission_requests),
            gate: Mutex::new(None),
        });
        let workflow = Arc::new(MailWorkflow::new(provider, cache));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, Some(requester), cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("cached source message is rendered");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-reply").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let reply_action = context
            .debug_bounds("message-reply")
            .expect("opened cached message exposes a native Reply action");
        context.simulate_click(reply_action.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        assert!(context.debug_bounds("mail-reply-context").is_some());
        assert!(context.debug_bounds("mail-recipient-input").is_none());
        assert!(context.debug_bounds("mail-subject-input").is_none());
        let body = context
            .debug_bounds("mail-body-input")
            .expect("reply composer exposes a plain-text body");
        context.simulate_click(body.center(), Modifiers::default());
        context.simulate_input("Synthetic private reply body");
        let send = context
            .debug_bounds("mail-send-button")
            .expect("reply composer exposes an explicit Send action");
        context.simulate_click(send.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("review-send-permission").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        let review = context
            .debug_bounds("review-send-permission")
            .expect("permission denial offers an explicit Mail.Send review");
        context.simulate_click(review.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        assert_eq!(permission_requests.load(Ordering::Relaxed), 1);
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);

        can_send.store(true, Ordering::Relaxed);
        let send = context
            .debug_bounds("mail-send-button")
            .expect("permission review leaves the reply available");
        context.simulate_click(send.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if send_calls.load(Ordering::Relaxed) == 2 {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }

        assert_eq!(send_calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            reply_sources
                .lock()
                .expect("fixture reply-source mutex is healthy")
                .as_slice(),
            [
                (
                    "synthetic-account".to_owned(),
                    "inbox".to_owned(),
                    message_id.as_str().to_owned(),
                ),
                (
                    "synthetic-account".to_owned(),
                    "inbox".to_owned(),
                    message_id.as_str().to_owned(),
                ),
            ]
        );
    }

    #[gpui_kit::test]
    async fn unknown_reply_requires_sent_items_review_and_duplicate_confirmation(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_mail_composer_keys(cx);
        });
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let send_calls = Arc::new(AtomicUsize::new(0));
        let reply_sources = Arc::new(Mutex::new(Vec::new()));
        let provider = PermissionProvider {
            offline: OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            can_send: Arc::new(AtomicBool::new(true)),
            send_calls: Arc::clone(&send_calls),
            send_gate: Arc::default(),
            reply_sources: Arc::clone(&reply_sources),
            reply_error: Arc::new(Mutex::new(Some(MailSendProviderError::OutcomeUnknown))),
        };
        let workflow = Arc::new(MailWorkflow::new(provider, cache));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let row = context
            .debug_bounds("message-row-message-1")
            .expect("cached source message is rendered");
        context.simulate_click(row.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("message-reply").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let reply_action = context
            .debug_bounds("message-reply")
            .expect("opened cached message exposes a native Reply action");
        context.simulate_click(reply_action.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        let body = context
            .debug_bounds("mail-body-input")
            .expect("reply composer exposes its body");
        context.simulate_click(body.center(), Modifiers::default());
        context.simulate_input("Synthetic private reply body");
        let send = context
            .debug_bounds("mail-send-button")
            .expect("reply composer exposes its Send action");
        context.simulate_click(send.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("mail-pending-send").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert!(context.debug_bounds("mail-pending-send").is_some());

        let send = context
            .debug_bounds("mail-send-button")
            .expect("unresolved reply keeps Send visible");
        context.simulate_click(send.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        let resolve = context
            .debug_bounds("resolve-pending-mail-send")
            .expect("unknown reply requires explicit Sent Items review");
        context.simulate_click(resolve.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("confirm-duplicate-risk").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert!(context.debug_bounds("mail-pending-send").is_none());
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        let confirm = context
            .debug_bounds("confirm-duplicate-risk")
            .expect("resolved unknown outcome requires duplicate-risk acknowledgement");
        context.simulate_click(confirm.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        let send = context
            .debug_bounds("mail-send-button")
            .expect("confirmed duplicate risk allows a separate reply");
        context.simulate_click(send.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if send_calls.load(Ordering::Relaxed) == 2 {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }

        assert_eq!(send_calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            reply_sources
                .lock()
                .expect("fixture reply-source mutex is healthy")
                .iter()
                .map(|source| source.2.as_str())
                .collect::<Vec<_>>(),
            [message_id.as_str(), message_id.as_str()]
        );
    }

    #[gpui_kit::test]
    async fn permission_review_requires_a_separate_send_action(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_mail_composer_keys(cx);
        });
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account.clone());
        let can_send = Arc::new(AtomicBool::new(false));
        let send_calls = Arc::new(AtomicUsize::new(0));
        let provider = PermissionProvider {
            offline: OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            can_send: Arc::clone(&can_send),
            send_calls: Arc::clone(&send_calls),
            send_gate: Arc::default(),
            reply_sources: Arc::default(),
            reply_error: Arc::default(),
        };
        let permission_requests = Arc::new(AtomicUsize::new(0));
        let (permission_started, permission_started_receiver) = tokio::sync::oneshot::channel();
        let (permission_release, permission_release_receiver) = tokio::sync::oneshot::channel();
        let requester: Arc<dyn MailPermissionRequester> = Arc::new(FixturePermissionRequester {
            account_id: account,
            requests: Arc::clone(&permission_requests),
            gate: Mutex::new(Some(PermissionGate {
                started: permission_started,
                release: permission_release_receiver,
            })),
        });
        let workflow = Arc::new(MailWorkflow::new(provider, cache));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, Some(requester), cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let compose = context
            .debug_bounds("new-mail-message")
            .expect("mail view exposes the compose action");
        context.simulate_click(compose.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        for (selector, value) in [
            ("mail-recipient-input", "ada@example.test"),
            ("mail-subject-input", "Synthetic subject"),
            ("mail-body-input", "Synthetic private body"),
        ] {
            let bounds = context
                .debug_bounds(selector)
                .expect("composer renders each message field");
            context.simulate_click(bounds.center(), Modifiers::default());
            context.simulate_input(value);
        }
        let send = context
            .debug_bounds("mail-send-button")
            .expect("composer exposes its explicit send action");
        context.simulate_click(send.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("review-send-permission").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let review = context
            .debug_bounds("review-send-permission")
            .expect("permission denial offers an explicit review action");
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        context.simulate_click(review.center(), Modifiers::default());
        permission_started_receiver
            .await
            .expect("permission review reaches its held requester");
        context.run_until_parked();
        draw_mail_view(context);
        context.simulate_keystrokes("escape");
        let remains_open_after_permission_escape =
            context.update(|window, cx| window.has_active_sheet(cx));
        click_composer_sheet_close(context);
        let remains_open_after_permission_close =
            context.update(|window, cx| window.has_active_sheet(cx));
        let _ = permission_release.send(());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.cx.update(|cx| {
                !view
                    .read(cx)
                    .composer
                    .as_ref()
                    .is_some_and(|composer| composer.read(cx).operation_in_progress())
            }) && context.debug_bounds("mail-composer-status").is_some()
            {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert!(remains_open_after_permission_escape);
        assert!(remains_open_after_permission_close);
        assert_eq!(permission_requests.load(Ordering::Relaxed), 1);
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        can_send.store(true, Ordering::Relaxed);
        context.cx.executor().timer(Duration::from_millis(20)).await;
        context.run_until_parked();
        draw_mail_view(context);
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);

        let send = context
            .debug_bounds("mail-send-button")
            .expect("permission review leaves the draft available");
        context.simulate_click(send.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        assert_eq!(send_calls.load(Ordering::Relaxed), 2);
    }

    #[gpui_kit::test]
    async fn pending_send_cannot_be_dismissed_by_escape_or_outside_click(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_mail_composer_keys(cx);
        });
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let provider = PermissionProvider {
            offline: OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            can_send: Arc::new(AtomicBool::new(true)),
            send_calls: Arc::new(AtomicUsize::new(0)),
            send_gate: Arc::default(),
            reply_sources: Arc::default(),
            reply_error: Arc::default(),
        };
        let (send_started, release_send) = provider.hold_next_send();
        let workflow = Arc::new(MailWorkflow::new(provider.clone(), cache));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let compose = context
            .debug_bounds("new-mail-message")
            .expect("mail view exposes the compose action");
        context.simulate_click(compose.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
        for (selector, value) in [
            ("mail-recipient-input", "ada@example.test"),
            ("mail-subject-input", "Synthetic subject"),
            ("mail-body-input", "Synthetic private body"),
        ] {
            let bounds = context
                .debug_bounds(selector)
                .expect("composer renders each message field");
            context.simulate_click(bounds.center(), Modifiers::default());
            context.simulate_input(value);
        }
        let send = context
            .debug_bounds("mail-send-button")
            .expect("composer exposes its explicit send action");
        context.simulate_click(send.center(), Modifiers::default());
        send_started
            .await
            .expect("synthetic send reaches its held provider");
        context.run_until_parked();
        draw_mail_view(context);

        let compose = context
            .debug_bounds("new-mail-message")
            .expect("mail view remains visible behind the composer");
        context.simulate_click(compose.center(), Modifiers::default());
        let remains_open_after_click = context.update(|window, cx| window.has_active_sheet(cx));
        context.simulate_keystrokes("escape");
        let remains_open_after_escape = context.update(|window, cx| window.has_active_sheet(cx));
        click_composer_sheet_close(context);
        let remains_open_after_title_close =
            context.update(|window, cx| window.has_active_sheet(cx));
        let _ = release_send.send(());

        assert!(remains_open_after_click);
        assert!(remains_open_after_escape);
        assert!(remains_open_after_title_close);
        assert_eq!(provider.send_calls.load(Ordering::Relaxed), 1);
    }

    #[gpui_kit::test]
    async fn send_staging_failure_skips_provider_and_accepted_cleanup_failure_can_be_resolved(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_mail_composer_keys(cx);
        });
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account.clone());
        cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .fail_next_pending_send_stage = true;
        let send_calls = Arc::new(AtomicUsize::new(0));
        let provider = PermissionProvider {
            offline: OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            can_send: Arc::new(AtomicBool::new(true)),
            send_calls: Arc::clone(&send_calls),
            send_gate: Arc::default(),
            reply_sources: Arc::default(),
            reply_error: Arc::default(),
        };
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let compose = context
            .debug_bounds("new-mail-message")
            .expect("mail view exposes the compose action");
        context.simulate_click(compose.center(), Modifiers::default());
        draw_mail_view(context);
        for (selector, value) in [
            ("mail-recipient-input", "ada@example.test"),
            ("mail-subject-input", "Synthetic subject"),
            ("mail-body-input", "Synthetic private body"),
        ] {
            let bounds = context
                .debug_bounds(selector)
                .expect("composer renders each message field");
            context.simulate_click(bounds.center(), Modifiers::default());
            context.simulate_input(value);
        }
        let send = context
            .debug_bounds("mail-send-button")
            .expect("composer exposes its explicit send action");
        context.simulate_click(send.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            let operation_in_progress = context.cx.update(|cx| {
                view.read(cx)
                    .composer
                    .as_ref()
                    .is_some_and(|composer| composer.read(cx).operation_in_progress())
            });
            if !operation_in_progress && context.debug_bounds("mail-composer-status").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);
        assert!(context.cx.update(|cx| view.read(cx).pending_send.is_none()));
        let send = context
            .debug_bounds("mail-send-button")
            .expect("staging failure leaves the draft available for another attempt");
        cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .fail_next_pending_send_clear = true;
        context.simulate_click(send.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.debug_bounds("resolve-pending-mail-send").is_some() {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        let resolve = context
            .debug_bounds("resolve-pending-mail-send")
            .expect("accepted send with a failed cleanup exposes an explicit resolution");
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert!(context.cx.update(|cx| view.read(cx).pending_send.is_some()));
        context.simulate_click(resolve.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if context.cx.update(|cx| view.read(cx).pending_send.is_none()) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }
        assert!(context.cx.update(|cx| view.read(cx).pending_send.is_none()));
        assert!(context.debug_bounds("resolve-pending-mail-send").is_none());
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
    }
}
