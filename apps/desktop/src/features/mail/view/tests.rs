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
    AppContext, Entity, Modifiers, MouseButton, ScrollStrategy, TestAppContext, VisualTestContext,
    point, px, size,
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

struct UnexpectedPermissionRequester;

impl MailPermissionRequester for UnexpectedPermissionRequester {
    fn request_send_permission<'a>(
        &'a self,
        _account_id: &'a AccountId,
    ) -> Pin<Box<dyn Future<Output = Result<(), AccountOperationError>> + Send + 'a>> {
        Box::pin(async {
            panic!("test did not expect a send permission request");
        })
    }
}

fn unexpected_permission_requester() -> Arc<dyn MailPermissionRequester> {
    Arc::new(UnexpectedPermissionRequester)
}

fn test_mail_view(
    profile: AccountProfile,
    workflow: Arc<dyn MailSession>,
    cx: &mut gpui_kit::Context<MailView>,
) -> MailView {
    MailView::new(profile, workflow, unexpected_permission_requester(), cx)
}

struct TestMailWorkspace {
    folder_navigation: Entity<MailFolderNavigation>,
    mail_view: Entity<MailView>,
}

impl gpui_kit::Render for TestMailWorkspace {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        gpui_kit::div()
            .size_full()
            .flex()
            .child(
                gpui_kit::div()
                    .w(gpui_kit::rems(16.))
                    .h_full()
                    .flex_shrink_0()
                    .child(self.folder_navigation.clone()),
            )
            .child(
                gpui_kit::div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.mail_view.clone()),
            )
    }
}

fn test_mail_workspace(
    profile: AccountProfile,
    workflow: Arc<dyn MailSession>,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::Context<gpui_kit::base::Root>,
) -> (gpui_kit::base::Root, Entity<MailView>) {
    let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
    let navigation = cx.new(|cx| MailFolderNavigation::new(view.clone(), cx));
    let workspace = cx.new(|_| TestMailWorkspace {
        folder_navigation: navigation,
        mail_view: view.clone(),
    });
    (gpui_kit::base::Root::new(workspace, window, cx), view)
}

fn mail_profile_with_capabilities(account: AccountId, mail: MailCapabilities) -> AccountProfile {
    AccountProfile::try_new_with_capabilities(
        account,
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::new(mail, CalendarAccess::ReadOnly),
    )
    .expect("synthetic account profile has an identity")
}

async fn wait_for_mail_sync_to_finish(context: &mut VisualTestContext, view: &Entity<MailView>) {
    wait_for_ui_condition(context, |_, app| {
        !view.read(app).sync_status.is_sync_active()
    })
    .await;
}

#[test]
fn sync_activity_matches_mail_status() {
    assert!(SyncStatus::LoadingCache.is_sync_active());
    assert!(SyncStatus::Syncing.is_sync_active());
    assert!(SyncStatus::Rebuilding.is_sync_active());
    assert!(!SyncStatus::Idle.is_sync_active());
    assert!(!SyncStatus::UpToDate.is_sync_active());
    assert!(!SyncStatus::Failed(MailWorkflowError::AccountMismatch).is_sync_active());
}

#[test]
fn active_mail_operation_explains_when_to_retry() {
    assert_eq!(
        mail_error_message(MailWorkflowError::OperationInProgress),
        "Another mail operation is active. Try again shortly."
    );
}

#[test]
fn empty_message_detail_copy_reflects_message_availability() {
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (_, cache, _) = fixture(account);
    let messages = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .messages
        .clone();
    let folder = messages
        .first()
        .expect("fixture contains a synthetic message")
        .folder()
        .clone();

    assert_eq!(
        empty_message_detail_copy(&MessageListState::Ready, Some(&folder), &messages),
        "Select a message to read it"
    );
    assert_eq!(
        empty_message_detail_copy(&MessageListState::Ready, Some(&folder), &[]),
        "No message selected"
    );
    assert_eq!(
        empty_message_detail_copy(&MessageListState::Ready, None, &messages),
        "No message selected"
    );
    assert_eq!(
        empty_message_detail_copy(&MessageListState::Loading, Some(&folder), &messages),
        "No message selected"
    );
    assert_eq!(
        empty_message_detail_copy(
            &MessageListState::Failed(MailWorkflowError::Provider(
                MailOperationError::ServiceUnavailable,
            )),
            Some(&folder),
            &messages,
        ),
        "No message selected"
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
    let message_id =
        MessageId::try_from("synthetic-message".to_owned()).expect("synthetic message ID is valid");
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

    fn message_delta_page(
        &self,
        _: &FolderReference,
        _: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
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
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
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
    folders: Option<Vec<MailFolder>>,
    sync_gate: Mutex<Option<SyncGate>>,
    offline: OfflineProvider,
}

struct SyncGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
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
            folders: None,
            sync_gate: Mutex::new(None),
            offline: OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
        }
    }

    fn with_folders(mut self, folders: Vec<MailFolder>) -> Self {
        self.folders = Some(folders);
        self
    }

    fn hold_sync_until_released(
        mut self,
        started: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) -> Self {
        self.sync_gate = Mutex::new(Some(SyncGate { started, release }));
        self
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
        let folders = self.folders.clone();
        let sync_gate = self
            .sync_gate
            .lock()
            .expect("fixture sync-gate mutex is healthy")
            .take();
        async move {
            if sync_failure {
                return Err(MailOperationError::ServiceUnavailable);
            }
            if let Some(sync_gate) = sync_gate {
                sync_gate
                    .started
                    .send(())
                    .expect("the mail sync test is waiting at its provider gate");
                sync_gate
                    .release
                    .await
                    .expect("the mail sync test releases its provider gate");
            }
            if let Some(folders) = folders {
                return Ok(folders);
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

    fn message_delta_page(
        &self,
        _: &FolderReference,
        _: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
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
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
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

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        _: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
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
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
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
                MessageMoveActionResponse::AccessDenied => Err(MailOperationError::AccessDenied),
                MessageMoveActionResponse::Unknown => Err(MailOperationError::MoveOutcomeUnknown),
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

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
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
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
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
                gate.started
                    .send(())
                    .expect("the mail send test is waiting at its provider gate");
                gate.release
                    .await
                    .expect("the mail send test releases its provider gate");
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
        let matches_account =
            account_id.as_str() == "synthetic-account" && draft.source().account_id() == account_id;
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
                gate.started
                    .send(())
                    .expect("the mail reply test is waiting at its provider gate");
                gate.release
                    .await
                    .expect("the mail reply test releases its provider gate");
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
                gate.started
                    .send(())
                    .expect("the permission review test is waiting at its requester gate");
                gate.release
                    .await
                    .expect("the permission review test releases its requester gate");
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
    list_folders_error: Option<MailCacheError>,
    list_folders_gate: Option<FolderListGate>,
    body: Option<MessageBody>,
    list_messages_error: Option<MailCacheError>,
    sync_state: Option<MailSyncState>,
    read_state_write_error: Option<MailCacheError>,
    folder_write_sync_required: bool,
    pending_send: Option<PendingMailSend>,
    fail_next_pending_send_stage: bool,
    fail_next_pending_send_clear: bool,
    move_error: Option<MailCacheError>,
}

struct FolderListGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

impl FixtureCache {
    fn hold_next_folder_list(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started, started_receiver) = tokio::sync::oneshot::channel();
        let (release, release_receiver) = tokio::sync::oneshot::channel();
        self.0
            .lock()
            .expect("fixture cache mutex is healthy")
            .list_folders_gate = Some(FolderListGate {
            started,
            release: release_receiver,
        });
        (started_receiver, release)
    }
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
    ) -> impl Future<Output = Result<Option<MailSyncState>, MailCacheError>> + Send + '_ {
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
        let Some(generation) = current.full_sync_generation().unwrap_or(0).checked_add(1) else {
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
            let (folders, error, gate) = {
                let mut data = self.0.lock().expect("fixture cache mutex is healthy");
                if data
                    .account_id
                    .as_ref()
                    .is_some_and(|saved| saved != &account_id)
                {
                    return Err(MailCacheError::AccountMismatch);
                }
                (
                    data.folders.clone(),
                    data.list_folders_error,
                    data.list_folders_gate.take(),
                )
            };
            if let Some(gate) = gate {
                gate.started
                    .send(())
                    .expect("the folder-list test is waiting at its cache gate");
                gate.release
                    .await
                    .expect("the folder-list test releases its cache gate");
            }
            if let Some(error) = error {
                return Err(error);
            }
            Ok(folders)
        }
    }

    fn list_messages(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Vec<MailMessage>, MailCacheError>> + Send + '_ {
        let folder = folder.clone();
        let data = self.0.lock().expect("fixture cache mutex is healthy");
        if let Some(error) = data.list_messages_error {
            return ready(Err(error));
        }
        let messages = data
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
    ) -> impl Future<Output = Result<Option<(MailMessage, Option<MessageBody>)>, MailCacheError>>
    + Send
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
        let Some(index) = data
            .messages
            .iter()
            .position(|message| message.folder() == source_folder && message.id() == message_id)
        else {
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
    ) -> impl Future<Output = Result<Option<PendingMailSend>, MailCacheError>> + Send + '_ {
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
}

impl AccountCache for FixtureCache {
    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + '_ {
        let mut data = self.0.lock().expect("fixture cache mutex is healthy");
        let result = match data.account_id.as_ref() {
            Some(saved) if saved != account_id => Err(AccountCacheRemovalError::AccountMismatch),
            _ => {
                *data = FixtureData::default();
                Ok(())
            }
        };
        ready(result)
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

fn message_list_width(context: &mut VisualTestContext) -> gpui_kit::Pixels {
    context.update(|window, _| window.find("mail-message-list").bounds().size.width)
}

fn open_cached_html_message(cx: &mut TestAppContext, content: String) -> &mut VisualTestContext {
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    cache.0.lock().expect("fixture cache mutex is healthy").body =
        Some(MessageBody::new(MessageBodyType::Html, content));
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (_, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    open_first_cached_message(context);
    context
}

fn open_first_cached_message(context: &mut VisualTestContext) {
    context.run_until_parked();
    draw_mail_view(context);
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
}

fn accessible_alert_count(context: &mut VisualTestContext, expected_text: &str) -> usize {
    context.update(|window, _| {
        window
            .find_all_by_role(gpui_kit::Role::Alert)
            .iter()
            .filter(|alert| alert.label() == Some(expected_text))
            .count()
    })
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
        let message_id =
            MessageId::try_from(format!("message-{index}")).expect("synthetic message ID is valid");
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

fn activate_focused_control(context: &mut VisualTestContext) {
    context.update(|window, cx| {
        window.key_down("enter", false, cx);
        window.key_up("enter", cx);
    });
}

const SYNTHETIC_MESSAGE_LABEL_PREFIX: &str = "Ada Example <ada@example.test>. Synthetic subject";

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
        .update(|cx| view.read(cx).selected_message_id().cloned());
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
async fn message_body_failure_keeps_the_cached_list_and_selection(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, message_id) = fixture(account);
    cache.0.lock().expect("fixture cache mutex is healthy").body = None;
    let provider = OfflineProvider {
        body_fetches: Arc::new(AtomicUsize::new(0)),
    };
    let body_fetches = Arc::clone(&provider.body_fetches);
    let workflow = Arc::new(MailWorkflow::new(profile.id().clone(), provider, cache));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    wait_for_mail_sync_to_finish(context, &view).await;

    let row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is visible before opening");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(
            &view.read(app).message_detail_state,
            MessageDetailState::Failed { .. }
        )
    })
    .await;

    assert!(context.debug_bounds("message-row-message-1").is_some());
    assert_eq!(
        accessible_alert_count(
            context,
            mail_error_message(MailWorkflowError::Provider(
                MailOperationError::ServiceUnavailable
            ))
        ),
        1,
        "the body-fetch failure appears once in the accessible alerts"
    );
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        matches!(&view.message_list_state, MessageListState::Ready)
            && matches!(
                &view.message_detail_state,
                MessageDetailState::Failed {
                    error: MailWorkflowError::Provider(_),
                    ..
                }
            )
            && view.selected_message_id() == Some(&message_id)
    }));
    assert_selected_message_and_accessible_label(
        context,
        &view,
        Some(&message_id),
        "Ada Example <ada@example.test>. Synthetic subject",
    );
    assert_eq!(body_fetches.load(Ordering::Relaxed), 1);
}

#[gpui_kit::test]
async fn folder_message_list_failure_does_not_claim_a_message_is_loading(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");

    wait_for_mail_sync_to_finish(context, &view).await;

    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .list_messages_error = Some(MailCacheError::Unavailable);
    let archive = context.update(|window, _| window.find_by_label("Archive"));
    context.simulate_click(archive.bounds().center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(
            &view.read(app).message_list_state,
            MessageListState::Failed(_)
        )
    })
    .await;

    assert_eq!(
        accessible_alert_count(
            context,
            mail_error_message(MailWorkflowError::Cache(MailCacheError::Unavailable))
        ),
        1,
        "the folder-list failure appears once in the accessible alerts"
    );
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        matches!(
            &view.message_list_state,
            MessageListState::Failed(MailWorkflowError::Cache(MailCacheError::Unavailable))
        ) && matches!(&view.message_detail_state, MessageDetailState::Empty)
    }));
}

#[gpui_kit::test]
fn offline_mail_view_opens_cached_message_without_provider_body_request(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, message_id) = fixture(account);
    let provider = OfflineProvider {
        body_fetches: Arc::new(AtomicUsize::new(0)),
    };
    let body_fetches = Arc::clone(&provider.body_fetches);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

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
            &view.read(cx).message_detail_state,
            MessageDetailState::Opened(opened)
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
async fn cached_folders_and_open_message_remain_available_during_sync(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_message_navigation_keys(cx);
    });
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, inbox_message_id) = fixture(account.clone());
    let archive_reference = FolderReference::new(
        account,
        FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
    );
    let archive_message_id =
        MessageId::try_from("archive-message".to_owned()).expect("synthetic message ID is valid");
    {
        let mut state = cache.0.lock().expect("fixture cache mutex is healthy");
        let sender = Mailbox::try_new(
            "ada@example.test".to_owned(),
            Some("Ada Example".to_owned()),
        )
        .expect("synthetic mailbox is valid");
        state.messages.push(MailMessage::new(
            archive_reference.clone(),
            archive_message_id.clone(),
            einfach_mail_domain::MessageMetadata::new(
                Some("Archived synthetic subject".to_owned()),
                einfach_mail_domain::MessageOrigin::new(Some(sender), None),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                ReadState::Unread,
                None,
            ),
        ));
    }
    let folders = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .clone();
    let inbox_reference = folders
        .iter()
        .find(|folder| folder.display_name() == "Inbox")
        .expect("fixture contains the inbox folder")
        .reference()
        .clone();
    let (sync_started, sync_started_receiver) = tokio::sync::oneshot::channel();
    let (resume_sync, sync_release) = tokio::sync::oneshot::channel();
    let provider = ReadStateProvider::new(
        ReadStateActionResponse::Confirm,
        Arc::new(Mutex::new(Vec::new())),
    )
    .with_folders(folders.clone())
    .hold_sync_until_released(sync_started, sync_release);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");

    sync_started_receiver
        .await
        .expect("account sync reaches its provider gate after loading the cache");
    assert!(context.cx.update(|app| {
        matches!(view.read(app).sync_status, SyncStatus::Syncing)
            && view.read(app).selected_folder.is_some()
    }));

    let archive_button = context.update(|window, _| window.find_by_label("Archive"));
    context.simulate_click(archive_button.bounds().center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        let view = view.read(app);
        view.selected_folder.as_ref() == Some(&archive_reference)
            && matches!(view.message_list_state, MessageListState::Ready)
    })
    .await;

    context.update(gpui_kit::Window::focus_next);
    let mut list_focused = false;
    for _ in 0..20 {
        press_window_key(context, "tab");
        list_focused =
            context.update(|window, app| view.read(app).message_list_focus.is_focused(window));
        if list_focused {
            break;
        }
    }
    assert!(
        list_focused,
        "Tab reaches cached folders' message list during sync"
    );
    press_window_key(context, "home");
    wait_for_ui_condition(context, |_, app| {
        matches!(
            view.read(app).selected_message_id(),
            Some(message_id) if message_id == &archive_message_id
        ) && matches!(
            &view.read(app).message_detail_state,
            MessageDetailState::Opened(opened) if opened.loaded_from_cache()
        )
    })
    .await;

    let (snapshot_list_started, snapshot_list_release) = cache.hold_next_folder_list();
    resume_sync
        .send(())
        .expect("the test releases the held mail sync");
    snapshot_list_started
        .await
        .expect("sync completion starts loading its selected-folder snapshot");

    let inbox_button = context.update(|window, _| window.find_by_label("Inbox"));
    context.simulate_click(inbox_button.bounds().center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        let view = view.read(app);
        view.selected_folder.as_ref() == Some(&inbox_reference)
            && matches!(view.message_list_state, MessageListState::Ready)
    })
    .await;
    let inbox_row = context
        .debug_bounds("message-row-message-1")
        .expect("the cached inbox message is visible");
    context.simulate_click(inbox_row.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(
            &view.read(app).message_detail_state,
            MessageDetailState::Opened(opened)
                if opened.message().id() == &inbox_message_id
                    && opened.loaded_from_cache()
        )
    })
    .await;
    snapshot_list_release
        .send(())
        .expect("the test releases the selected-folder snapshot");

    wait_for_mail_sync_to_finish(context, &view).await;
    wait_for_ui_condition(context, |_, app| {
        let view = view.read(app);
        view.selected_folder.as_ref() == Some(&inbox_reference)
            && matches!(
                &view.message_detail_state,
                MessageDetailState::Opened(opened)
                    if opened.message().id() == &inbox_message_id
                        && opened.loaded_from_cache()
            )
    })
    .await;
}

#[gpui_kit::test]
async fn contextual_folder_navigation_updates_mail_selection(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    let inbox_reference = FolderReference::new(
        profile.id().clone(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic inbox ID is valid"),
    );
    let archive_reference = FolderReference::new(
        profile.id().clone(),
        FolderId::try_from("archive".to_owned()).expect("synthetic archive ID is valid"),
    );
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");
    context.run_until_parked();
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);

    assert!(
        context
            .update(|_, app| { view.read(app).selected_folder.as_ref() == Some(&inbox_reference) })
    );
    let archive_button = context.update(|window, _| window.find_by_label("Archive"));
    context.simulate_click(archive_button.bounds().center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        view.read(app).selected_folder.as_ref() == Some(&archive_reference)
    })
    .await;
    draw_mail_view(context);

    assert!(context.update(|window, _| {
        window.try_find_by_label("Inbox").is_some() && window.try_find_by_label("Archive").is_some()
    }));
}

#[gpui_kit::test]
async fn initial_mail_snapshot_renders_loading_without_selected_folder(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    let (started_receiver, release) = cache.hold_next_folder_list();
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    started_receiver
        .await
        .expect("initial folder listing reaches its test gate");

    assert!(context.cx.update(|cx| {
        view.read(cx).selected_folder.is_none()
            && matches!(&view.read(cx).message_list_state, MessageListState::Loading)
    }));
    draw_mail_view(context);

    assert!(context.debug_bounds("mail-list-loading").is_some());
    release
        .send(())
        .expect("the initial folder listing is waiting for release");
    context.run_until_parked();
}

#[gpui_kit::test]
fn initial_mail_snapshot_failure_renders_error_without_selected_folder(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .list_folders_error = Some(MailCacheError::Unavailable);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    context.run_until_parked();
    draw_mail_view(context);

    assert!(context.cx.update(|cx| {
        view.read(cx).selected_folder.is_none()
            && matches!(
                &view.read(cx).message_list_state,
                MessageListState::Failed(MailWorkflowError::Cache(MailCacheError::Unavailable))
            )
    }));
    assert!(context.debug_bounds("mail-list-error").is_none());
    assert!(context.debug_bounds("mail-list-empty-folders").is_none());
    let error_announcement_counts = context.update(|window, _| {
        let message = mail_error_message(MailWorkflowError::Cache(MailCacheError::Unavailable));
        let count_matching_message = |role| {
            window
                .find_all_by_role(role)
                .iter()
                .filter(|announcement| announcement.label() == Some(message))
                .count()
        };
        (
            count_matching_message(gpui_kit::Role::Alert),
            count_matching_message(gpui_kit::Role::Status),
        )
    });
    assert_eq!(
        error_announcement_counts,
        (1, 0),
        "the initial cache error appears once in the header alert and not in the empty list"
    );
}

#[gpui_kit::test]
fn successful_empty_mail_snapshot_renders_no_folders(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .clear();
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    context.run_until_parked();
    draw_mail_view(context);

    assert!(context.cx.update(|cx| {
        view.read(cx).selected_folder.is_none()
            && matches!(&view.read(cx).message_list_state, MessageListState::Ready)
    }));
    assert!(context.debug_bounds("mail-list-empty-folders").is_some());
    assert!(context.debug_bounds("mail-list-loading").is_none());
    assert!(context.debug_bounds("mail-list-error").is_none());
}

#[gpui_kit::test]
fn cache_snapshot_failure_survives_a_successful_sync(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    context.run_until_parked();

    context.cx.update(|cx| {
        view.update(cx, |view, _| {
            let _ = view.finish_sync(
                Ok(()),
                Err(MailWorkflowError::Cache(MailCacheError::Unavailable)),
                None,
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
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
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
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
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
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    context.run_until_parked();

    let status = context.cx.update(|cx| view.read(cx).sync_status_label());

    assert!(status.contains("saved mail rebuild is incomplete"));
    assert!(status.contains("Choose Sync to resume it."));
}

mod body_rendering;
mod composer;
mod follow_up_flag;
mod message_actions;
mod message_list;
mod message_move;
mod read_state;
