use std::{
    future::{Future, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime},
};

use tokio::sync::oneshot;

use gpui_kit::{
    AppContext as _, Modifiers, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point,
    px, size,
};

use super::{AccountContent, AccountStatus, AccountView, SetupIssue};
use einfach_mail_application::{
    AccountCacheOpenError, AccountCacheRemovalError, AccountOperationCancellation,
    AccountOperationError, AccountPermissionRequest, AccountProvider, AccountSession,
    AccountSessionFactory, AccountSessionFuture, AccountWorkflow, CalendarCacheError,
    CalendarEventCreationOutcome, CalendarEventDeletionOutcome, CalendarEventUpdateOutcome,
    CalendarSession, CalendarSnapshot, CalendarSyncReport, CalendarWorkflowError,
    FollowUpFlagUpdateOutcome, MailCacheError, MailSendOutcome, MailSendResolutionError,
    MailSendWorkflowError, MailSession, MailSyncProgress, MailSyncReport, MailSyncState,
    MailWorkflowError, MessageMoveOutcome, ReadStateUpdateOutcome, WellKnownFolderMoveOutcome,
    WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, CalendarEventCreation, CalendarEventUpdate, CalendarOccurrence,
    CalendarViewReference, EditableCalendarEvent, FolderReference, FollowUpFlagStatus, MailDraft,
    MailFolder, MailMessage, MailReplyDraft, MailSendOperationId, MessageId, PendingMailSend,
    ReadState,
};

#[derive(Clone, Default)]
struct TestProvider {
    signed_out: Arc<AtomicBool>,
    fail_next_sign_out: Arc<AtomicBool>,
    pause_connection: Arc<AtomicBool>,
    pause_permission_update: Arc<AtomicBool>,
    restore_error: Arc<Mutex<Option<AccountOperationError>>>,
    restore_profile: Arc<Mutex<Option<AccountProfile>>>,
    connect_error: Arc<Mutex<Option<AccountOperationError>>>,
    saved_account_id: Arc<Mutex<Option<AccountId>>>,
    sign_out_events: Arc<Mutex<Vec<&'static str>>>,
    connection_commit: Arc<Mutex<Option<CommitGate>>>,
    permission_commit: Arc<Mutex<Option<CommitGate>>>,
    last_permission_request: Arc<Mutex<Option<AccountPermissionRequest>>>,
}

struct CommitGate {
    started: mpsc::Sender<()>,
    finish: oneshot::Receiver<()>,
}

struct TestAccountSession {
    account_id: AccountId,
    removal_attempts: Arc<Mutex<Vec<AccountId>>>,
}

impl AccountSession for TestAccountSession {
    fn mail_session(&self) -> Arc<dyn MailSession> {
        Arc::new(TestMailSession)
    }

    fn calendar_session(&self) -> Arc<dyn CalendarSession> {
        Arc::new(TestCalendarSession)
    }

    fn remove_account_cache(
        &self,
    ) -> AccountSessionFuture<'_, Result<(), AccountCacheRemovalError>> {
        let attempts = Arc::clone(&self.removal_attempts);
        let account_id = self.account_id.clone();
        Box::pin(async move {
            let mut attempts = attempts
                .lock()
                .expect("synthetic session removal log mutex is healthy");
            attempts.push(account_id);
            if attempts.len() == 1 {
                Err(AccountCacheRemovalError::OperationInProgress)
            } else {
                Ok(())
            }
        })
    }
}

struct TestMailSession;

fn pending_mail_call<'a, Output: Send + 'a>() -> AccountSessionFuture<'a, Output> {
    Box::pin(std::future::pending())
}

impl MailSession for TestMailSession {
    fn synchronize_account<'a>(
        &'a self,
        _: SystemTime,
        _: &'a (dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> AccountSessionFuture<'a, Result<MailSyncReport, MailWorkflowError>> {
        pending_mail_call()
    }

    fn list_cached_folders(
        &self,
    ) -> AccountSessionFuture<'_, Result<Vec<MailFolder>, MailWorkflowError>> {
        pending_mail_call()
    }

    fn list_cached_messages<'a>(
        &'a self,
        _: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<Vec<MailMessage>, MailWorkflowError>> {
        pending_mail_call()
    }

    fn cached_sync_state<'a>(
        &'a self,
        _: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<Option<MailSyncState>, MailWorkflowError>> {
        pending_mail_call()
    }

    fn folder_write_sync_required<'a>(
        &'a self,
        _: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<bool, MailWorkflowError>> {
        pending_mail_call()
    }

    fn open_message<'a>(
        &'a self,
        _: &'a FolderReference,
        _: &'a MessageId,
    ) -> AccountSessionFuture<'a, Result<einfach_mail_application::OpenedMessage, MailWorkflowError>>
    {
        pending_mail_call()
    }

    fn set_message_read_state<'a>(
        &'a self,
        _: &'a FolderReference,
        _: &'a MessageId,
        _: ReadState,
    ) -> AccountSessionFuture<'a, Result<ReadStateUpdateOutcome, MailWorkflowError>> {
        pending_mail_call()
    }

    fn set_message_follow_up_flag_status<'a>(
        &'a self,
        _: &'a FolderReference,
        _: &'a MessageId,
        _: FollowUpFlagStatus,
    ) -> AccountSessionFuture<'a, Result<FollowUpFlagUpdateOutcome, MailWorkflowError>> {
        pending_mail_call()
    }

    fn move_message<'a>(
        &'a self,
        _: &'a FolderReference,
        _: &'a MessageId,
        _: &'a FolderReference,
    ) -> AccountSessionFuture<'a, Result<MessageMoveOutcome, MailWorkflowError>> {
        pending_mail_call()
    }

    fn move_message_to_well_known_folder<'a>(
        &'a self,
        _: &'a FolderReference,
        _: &'a MessageId,
        _: WellKnownMailFolder,
    ) -> AccountSessionFuture<'a, Result<WellKnownFolderMoveOutcome, MailWorkflowError>> {
        pending_mail_call()
    }

    fn send_message<'a>(
        &'a self,
        _: MailDraft,
        _: SystemTime,
    ) -> AccountSessionFuture<'a, Result<MailSendOutcome, MailSendWorkflowError>> {
        pending_mail_call()
    }

    fn reply_to_message<'a>(
        &'a self,
        _: MailReplyDraft,
        _: SystemTime,
    ) -> AccountSessionFuture<'a, Result<MailSendOutcome, MailSendWorkflowError>> {
        pending_mail_call()
    }

    fn load_pending_send(
        &self,
    ) -> AccountSessionFuture<'_, Result<Option<PendingMailSend>, MailCacheError>> {
        pending_mail_call()
    }

    fn resolve_pending_send<'a>(
        &'a self,
        _: MailSendOperationId,
    ) -> AccountSessionFuture<'a, Result<(), MailSendResolutionError>> {
        pending_mail_call()
    }
}

struct TestCalendarSession;

impl CalendarSession for TestCalendarSession {
    fn load_cached_view<'a>(
        &'a self,
        _: &'a CalendarViewReference,
    ) -> AccountSessionFuture<'a, Result<CalendarSnapshot, CalendarWorkflowError>> {
        Box::pin(ready(Err(CalendarWorkflowError::Cache(
            CalendarCacheError::Unavailable,
        ))))
    }

    fn synchronize_view<'a>(
        &'a self,
        _: &'a CalendarViewReference,
    ) -> AccountSessionFuture<'a, Result<CalendarSyncReport, CalendarWorkflowError>> {
        Box::pin(ready(Err(CalendarWorkflowError::Cache(
            CalendarCacheError::Unavailable,
        ))))
    }

    fn editable_event<'a>(
        &'a self,
        _: &'a CalendarOccurrence,
    ) -> AccountSessionFuture<'a, Result<EditableCalendarEvent, CalendarWorkflowError>> {
        Box::pin(ready(Err(CalendarWorkflowError::Cache(
            CalendarCacheError::Unavailable,
        ))))
    }

    fn create_event<'a>(
        &'a self,
        _: &'a CalendarEventCreation,
    ) -> AccountSessionFuture<'a, Result<CalendarEventCreationOutcome, CalendarWorkflowError>> {
        Box::pin(ready(Err(CalendarWorkflowError::Cache(
            CalendarCacheError::Unavailable,
        ))))
    }

    fn discard_rejected_event_creation<'a>(
        &'a self,
        _: &'a CalendarEventCreation,
    ) -> AccountSessionFuture<'a, Result<(), CalendarWorkflowError>> {
        Box::pin(ready(Err(CalendarWorkflowError::Cache(
            CalendarCacheError::Unavailable,
        ))))
    }

    fn update_event<'a>(
        &'a self,
        _: &'a CalendarEventUpdate,
    ) -> AccountSessionFuture<'a, Result<CalendarEventUpdateOutcome, CalendarWorkflowError>> {
        Box::pin(ready(Err(CalendarWorkflowError::Cache(
            CalendarCacheError::Unavailable,
        ))))
    }

    fn delete_event<'a>(
        &'a self,
        _: &'a EditableCalendarEvent,
    ) -> AccountSessionFuture<'a, Result<CalendarEventDeletionOutcome, CalendarWorkflowError>> {
        Box::pin(ready(Err(CalendarWorkflowError::Cache(
            CalendarCacheError::Unavailable,
        ))))
    }
}

struct TestAccountSessionFactory {
    session: Arc<dyn AccountSession>,
    fallback_removals: Arc<AtomicBool>,
    session_open_attempts: Arc<AtomicUsize>,
    opened_account_ids: Arc<Mutex<Vec<AccountId>>>,
}

struct FailingOpenAccountSessionFactory {
    open_error: AccountCacheOpenError,
    open_gate: Mutex<Option<OpenSessionGate>>,
    sign_out_events: Arc<Mutex<Vec<&'static str>>>,
    removal_account_ids: Arc<Mutex<Vec<AccountId>>>,
}

struct OpenSessionGate {
    started: mpsc::Sender<()>,
    release: oneshot::Receiver<()>,
}

impl AccountSessionFactory for TestAccountSessionFactory {
    fn open_session(
        &self,
        account_id: AccountId,
    ) -> AccountSessionFuture<'_, Result<Arc<dyn AccountSession>, AccountCacheOpenError>> {
        self.session_open_attempts.fetch_add(1, Ordering::SeqCst);
        self.opened_account_ids
            .lock()
            .expect("synthetic opened-account ID mutex is healthy")
            .push(account_id);
        Box::pin(ready(Ok(Arc::clone(&self.session))))
    }

    fn remove_account_cache<'a>(
        &'a self,
        _: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), AccountCacheRemovalError>> {
        self.fallback_removals.store(true, Ordering::SeqCst);
        Box::pin(ready(Err(AccountCacheRemovalError::Unavailable)))
    }
}

impl AccountSessionFactory for FailingOpenAccountSessionFactory {
    fn open_session(
        &self,
        _: AccountId,
    ) -> AccountSessionFuture<'_, Result<Arc<dyn AccountSession>, AccountCacheOpenError>> {
        let open_error = self.open_error;
        let open_gate = self
            .open_gate
            .lock()
            .expect("synthetic open-session gate mutex is healthy")
            .take();
        Box::pin(async move {
            if let Some(gate) = open_gate {
                gate.started
                    .send(())
                    .expect("the synthetic open-session test is waiting");
                gate.release
                    .await
                    .expect("the synthetic open-session test releases the gate");
            }
            Err(open_error)
        })
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), AccountCacheRemovalError>> {
        self.sign_out_events
            .lock()
            .expect("synthetic sign-out event log mutex is healthy")
            .push("cache_removed");
        self.removal_account_ids
            .lock()
            .expect("synthetic cache removal ID mutex is healthy")
            .push(account_id.clone());
        Box::pin(ready(Ok(())))
    }
}

impl TestProvider {
    fn pause_next_connection_commit(&self) -> (mpsc::Receiver<()>, oneshot::Sender<()>) {
        Self::pause_next_commit(&self.connection_commit)
    }

    fn pause_next_permission_commit(&self) -> (mpsc::Receiver<()>, oneshot::Sender<()>) {
        Self::pause_next_commit(&self.permission_commit)
    }

    fn pause_next_commit(
        slot: &Mutex<Option<CommitGate>>,
    ) -> (mpsc::Receiver<()>, oneshot::Sender<()>) {
        let (started_sender, started_receiver) = mpsc::channel();
        let (finish_sender, finish_receiver) = oneshot::channel();
        *slot
            .lock()
            .expect("synthetic commit gate mutex is not poisoned") = Some(CommitGate {
            started: started_sender,
            finish: finish_receiver,
        });
        (started_receiver, finish_sender)
    }
}

impl AccountProvider for TestProvider {
    fn restore_account(
        &self,
    ) -> impl Future<Output = Result<Option<AccountProfile>, AccountOperationError>> + Send + '_
    {
        let restore_error = *self
            .restore_error
            .lock()
            .expect("synthetic restore error mutex is healthy");
        let restore_profile = self
            .restore_profile
            .lock()
            .expect("synthetic restored profile mutex is healthy")
            .clone();
        ready(restore_error.map_or(Ok(restore_profile), Err))
    }

    fn saved_credential_account_id(
        &self,
    ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_ {
        let account_id = self
            .saved_account_id
            .lock()
            .expect("synthetic saved account ID mutex is healthy")
            .clone();
        ready(Ok(account_id))
    }

    fn connect_account(
        &self,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        let pause_connection = Arc::clone(&self.pause_connection);
        let connection_error = self
            .connect_error
            .lock()
            .expect("synthetic connection error mutex is healthy")
            .take();
        let saved_account_id = Arc::clone(&self.saved_account_id);
        let signed_out = Arc::clone(&self.signed_out);
        let commit_gate = self
            .connection_commit
            .lock()
            .expect("synthetic commit gate mutex is not poisoned")
            .take();
        let profile = AccountProfile::try_new(
            AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is non-empty"),
            Some("Ada Example".to_owned()),
            Some("ada@example.test".to_owned()),
            Some("ada@tenant.example.test".to_owned()),
        )
        .expect("synthetic account profile has an identity");
        let account_id = profile.id().clone();
        async move {
            if let Some(error) = connection_error {
                return Err(error);
            }
            if pause_connection.load(Ordering::SeqCst) {
                cancellation.cancelled().await;
                return Err(AccountOperationError::SignInCancelled);
            }
            cancellation.begin_credential_commit()?;
            if let Some(commit_gate) = commit_gate {
                let _ = commit_gate.started.send(());
                commit_gate
                    .finish
                    .await
                    .map_err(|_| AccountOperationError::ServiceUnavailable)?;
            }
            *saved_account_id
                .lock()
                .expect("synthetic saved account ID mutex is healthy") = Some(account_id);
            signed_out.store(false, Ordering::SeqCst);
            Ok(profile)
        }
    }

    fn reauthorize_account(
        &self,
        expected_account_id: &AccountId,
        request: AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        *self
            .last_permission_request
            .lock()
            .expect("synthetic permission request mutex is not poisoned") = Some(request);
        let pause_permission_update = Arc::clone(&self.pause_permission_update);
        let commit_gate = self
            .permission_commit
            .lock()
            .expect("synthetic commit gate mutex is not poisoned")
            .take();
        let expected_account_id = expected_account_id.clone();
        let profile = AccountProfile::try_new(
            AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is non-empty"),
            Some("Ada Example".to_owned()),
            Some("ada@example.test".to_owned()),
            Some("ada@tenant.example.test".to_owned()),
        )
        .expect("synthetic account profile has an identity");
        async move {
            if pause_permission_update.load(Ordering::SeqCst) {
                cancellation.cancelled().await;
                return Err(AccountOperationError::SignInCancelled);
            }
            if expected_account_id != *profile.id() {
                return Err(AccountOperationError::AccountMismatch);
            }
            cancellation.begin_credential_commit()?;
            if let Some(commit_gate) = commit_gate {
                let _ = commit_gate.started.send(());
                commit_gate
                    .finish
                    .await
                    .map_err(|_| AccountOperationError::ServiceUnavailable)?;
            }
            Ok(profile)
        }
    }

    fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
        if self.fail_next_sign_out.swap(false, Ordering::SeqCst) {
            return ready(Err(AccountOperationError::CredentialStoreUnavailable));
        }
        self.sign_out_events
            .lock()
            .expect("synthetic sign-out event log mutex is healthy")
            .push("credential_removed");
        *self
            .saved_account_id
            .lock()
            .expect("synthetic saved account ID mutex is healthy") = None;
        self.signed_out.store(true, Ordering::SeqCst);
        ready(Ok(()))
    }
}

fn draw_account_view(context: &mut VisualTestContext) {
    context.update(|window, app| {
        let _ = window.draw(app);
    });
}

fn scroll_account_action_into_view(context: &mut VisualTestContext, selector: &'static str) {
    let Some(mut bounds) = context.debug_bounds(selector) else {
        panic!("account action `{selector}` is not rendered");
    };
    let viewport = context.update(|window, _| window.bounds());
    for _ in 0..4 {
        if bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom() {
            break;
        }
        let delta_y = if bounds.top() < viewport.top() {
            1_200.
        } else {
            -1_200.
        };
        scroll_account_view(context, delta_y);
        bounds = context
            .debug_bounds(selector)
            .expect("account action remains rendered while scrolling");
    }
    assert!(
        bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom(),
        "account action `{selector}` can be scrolled into view"
    );
}

fn click(context: &mut VisualTestContext, selector: &'static str) {
    scroll_account_action_into_view(context, selector);
    let bounds = context
        .debug_bounds(selector)
        .expect("visible account action remains rendered");
    context.simulate_click(bounds.center(), Modifiers::default());
    context.run_until_parked();
    draw_account_view(context);
}

fn scroll_account_view(context: &mut VisualTestContext, delta_y: f32) {
    let viewport = context.update(|window, _| window.bounds());
    context.simulate_event(ScrollEvent {
        position: viewport.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(delta_y))),
        ..Default::default()
    });
    draw_account_view(context);
}

#[test]
fn cancelled_permission_review_has_permission_specific_copy() {
    assert_eq!(
        AccountStatus::reauthorization_error_message(AccountOperationError::SignInCancelled),
        "Permission review was cancelled."
    );
}

#[test]
fn in_use_account_cache_message_explains_how_to_retry() {
    let message = super::account_cache_open_error_message(AccountCacheOpenError::InUse);

    assert!(message.contains("Another Einfach Mail operation"));
    assert!(message.contains("close the other account view"));
    assert!(message.contains("try again"));
}

#[test]
fn unavailable_account_cache_message_covers_file_and_keyring_recovery() {
    let message = super::account_cache_open_error_message(AccountCacheOpenError::Unavailable);

    assert!(message.contains("cache files"));
    assert!(message.contains("Linux keyring"));
    assert!(message.contains("try again"));
    assert!(!message.contains("removed"));
}

#[test]
fn cache_removal_validation_failure_explains_sign_in_remains() {
    for error in [
        AccountCacheRemovalError::InvalidData,
        AccountCacheRemovalError::AccountMismatch,
    ] {
        let message = super::account_cache_removal_error_message(error);

        assert!(message.contains("verified for removal"));
        assert!(message.contains("still signed in"));
    }
}

#[gpui_kit::test]
fn calendar_cache_open_failure_uses_account_scoped_error_copy(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::InvalidData,
        open_gate: Mutex::new(None),
        sign_out_events: Arc::clone(&provider.sign_out_events),
        removal_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let (view, context) =
        cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));
    context.run_until_parked();

    let profile = AccountProfile::try_new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
    )
    .expect("synthetic account profile is valid");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(profile);
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "open-calendar");

    assert!(context.debug_bounds("account-cache-error").is_some());
    assert!(context.cx.update(|cx| matches!(
        view.read(cx).session_open_error,
        Some(super::AccountViewCacheError::Open(
            AccountCacheOpenError::InvalidData
        ))
    )));
    let message = super::account_cache_open_error_message(AccountCacheOpenError::InvalidData);
    assert!(message.contains("account cache"));
    assert!(!message.contains("mail"));

    click(context, "review-app-permissions");
    assert!(
        context
            .cx
            .update(|cx| { view.read(cx).pending_permission_review.is_some() })
    );
    click(context, "cancel-permission-review");
    assert!(
        context
            .cx
            .update(|cx| { view.read(cx).pending_permission_review.is_none() })
    );
}

#[gpui_kit::test]
fn stale_content_open_failure_does_not_replace_a_reconnected_account(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let (open_started, open_started_receiver) = mpsc::channel();
    let (release_open, open_release) = oneshot::channel();
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::Unavailable,
        open_gate: Mutex::new(Some(OpenSessionGate {
            started: open_started,
            release: open_release,
        })),
        sign_out_events: Arc::new(Mutex::new(Vec::new())),
        removal_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));
    context.run_until_parked();
    let profile = AccountProfile::try_new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
    )
    .expect("synthetic account profile is valid");
    let replacement_account_id = AccountId::try_from("replacement-account".to_owned())
        .expect("replacement account ID is valid");
    let replacement_profile = AccountProfile::try_new(
        replacement_account_id.clone(),
        Some("Lin Example".to_owned()),
        Some("lin@example.test".to_owned()),
        Some("lin@tenant.example.test".to_owned()),
    )
    .expect("replacement account profile is valid");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(profile);
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "open-mail");
    open_started_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("the account session open starts");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.content = AccountContent::Opening(super::OpeningContent::Mail);
            view.status = AccountStatus::Connected(replacement_profile);
            cx.notify();
        });
    });
    release_open
        .send(())
        .expect("the synthetic account session open is waiting on this test");
    context.run_until_parked();

    let stale_error_ignored = context.cx.update(|cx| {
        let view = view.read(cx);
        matches!(
            &view.status,
            AccountStatus::Connected(profile) if profile.id() == &replacement_account_id
        ) && matches!(
            view.content,
            AccountContent::Opening(super::OpeningContent::Mail)
        ) && view.session_open_error.is_none()
    });
    assert!(stale_error_ignored);
}

#[gpui_kit::test]
fn connects_and_signs_out_through_the_account_view(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = provider.signed_out.clone();
    let sign_out_events = Arc::clone(&provider.sign_out_events);
    let event_log = Arc::clone(&sign_out_events);
    let removal_account_ids = Arc::new(Mutex::new(Vec::new()));
    provider.fail_next_sign_out.store(true, Ordering::SeqCst);
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::Unavailable,
        open_gate: Mutex::new(None),
        sign_out_events: Arc::clone(&sign_out_events),
        removal_account_ids: Arc::clone(&removal_account_ids),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));

    context.run_until_parked();
    draw_account_view(context);
    assert!(context.debug_bounds("connect-account").is_some());
    click(context, "connect-account");
    let connected = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::Connected(profile) if profile.label() == "Ada Example"
        )
    });
    assert!(connected);

    click(context, "sign-out");
    let retryable_failure = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::SignOutFailed {
                error: AccountOperationError::CredentialStoreUnavailable,
            }
        )
    });
    assert!(retryable_failure);
    assert!(context.debug_bounds("sign-out").is_some());
    assert!(!signed_out.load(Ordering::SeqCst));

    click(context, "sign-out");
    let disconnected = context
        .cx
        .update(|cx| matches!(&view.read(cx).status, super::AccountStatus::Disconnected));
    assert!(disconnected);
    assert!(signed_out.load(Ordering::SeqCst));
    assert_eq!(
        *removal_account_ids
            .lock()
            .expect("synthetic cache removal ID mutex is healthy"),
        vec![
            AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is valid");
            2
        ]
    );
    assert_eq!(
        *event_log
            .lock()
            .expect("synthetic sign-out event log mutex is healthy"),
        vec!["cache_removed", "cache_removed", "credential_removed"]
    );
}

#[gpui_kit::test]
fn failed_restore_removes_cache_before_the_verified_saved_credential(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = Arc::clone(&provider.signed_out);
    *provider
        .restore_error
        .lock()
        .expect("synthetic restore error mutex is healthy") =
        Some(AccountOperationError::SignInExpired);
    let account_id = AccountId::try_from("verified-saved-account".to_owned()).expect("ID is valid");
    *provider
        .saved_account_id
        .lock()
        .expect("synthetic saved account ID mutex is healthy") = Some(account_id.clone());
    let event_log = Arc::clone(&provider.sign_out_events);
    let removal_account_ids = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::Unavailable,
        open_gate: Mutex::new(None),
        sign_out_events: Arc::clone(&event_log),
        removal_account_ids: Arc::clone(&removal_account_ids),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));

    context.run_until_parked();
    draw_account_view(context);
    assert!(context.debug_bounds("connect-account").is_none());
    click(context, "sign-out");

    assert_eq!(
        *removal_account_ids
            .lock()
            .expect("synthetic cache removal ID mutex is healthy"),
        vec![account_id]
    );
    assert_eq!(
        *event_log
            .lock()
            .expect("synthetic sign-out event log mutex is healthy"),
        vec!["cache_removed", "credential_removed"]
    );
    assert!(signed_out.load(Ordering::SeqCst));
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).status, AccountStatus::Disconnected))
    );
}

#[gpui_kit::test]
fn failed_restore_without_verified_identity_preserves_credential(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = Arc::clone(&provider.signed_out);
    *provider
        .restore_error
        .lock()
        .expect("synthetic restore error mutex is healthy") =
        Some(AccountOperationError::SignInExpired);
    let event_log = Arc::clone(&provider.sign_out_events);
    let removal_account_ids = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::Unavailable,
        open_gate: Mutex::new(None),
        sign_out_events: Arc::clone(&event_log),
        removal_account_ids: Arc::clone(&removal_account_ids),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));

    context.run_until_parked();
    click(context, "sign-out");

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).status,
        AccountStatus::Failed {
            error: AccountOperationError::SavedAccountIdentityUnavailable,
            saved_credential_may_exist: true,
        }
    )));
    assert!(context.debug_bounds("connect-account").is_none());
    assert!(context.debug_bounds("sign-out").is_none());
    assert!(context.debug_bounds("retry-account-restore").is_some());
    assert!(
        removal_account_ids
            .lock()
            .expect("synthetic cache removal ID mutex is healthy")
            .is_empty()
    );
    assert!(
        event_log
            .lock()
            .expect("synthetic sign-out event log mutex is healthy")
            .is_empty()
    );
    assert!(!signed_out.load(Ordering::SeqCst));
}

#[gpui_kit::test]
fn connect_mismatch_keeps_saved_sign_in_and_requires_restore(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    *provider
        .saved_account_id
        .lock()
        .expect("synthetic saved account ID mutex is healthy") =
        Some(AccountId::try_from("existing-account".to_owned()).expect("account ID is valid"));
    *provider
        .connect_error
        .lock()
        .expect("synthetic connection error mutex is healthy") =
        Some(AccountOperationError::AccountMismatch);
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let (view, context) = cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), None, cx));

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-account");

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).status,
        AccountStatus::Failed {
            error: AccountOperationError::AccountMismatch,
            saved_credential_may_exist: true,
        }
    )));
    assert!(context.debug_bounds("connect-account").is_none());
    assert!(context.debug_bounds("retry-account-restore").is_some());
}

#[gpui_kit::test]
fn restore_to_another_account_opens_and_removes_its_session(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = Arc::clone(&provider.signed_out);
    let account_a_id = AccountId::try_from("account-a".to_owned()).expect("account ID is valid");
    let account_b_id = AccountId::try_from("account-b".to_owned()).expect("account ID is valid");
    let profile_a = AccountProfile::try_new(
        account_a_id.clone(),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
    )
    .expect("synthetic account profile is valid");
    let profile_b = AccountProfile::try_new(
        account_b_id.clone(),
        Some("Lin Example".to_owned()),
        Some("lin@example.test".to_owned()),
        Some("lin@tenant.example.test".to_owned()),
    )
    .expect("synthetic account profile is valid");
    let session_a_removals = Arc::new(Mutex::new(Vec::new()));
    let session_b_removals = Arc::new(Mutex::new(Vec::new()));
    let session_a: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: account_a_id,
        removal_attempts: Arc::clone(&session_a_removals),
    });
    let session_b: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: account_b_id.clone(),
        removal_attempts: Arc::clone(&session_b_removals),
    });
    let session_open_attempts = Arc::new(AtomicUsize::new(0));
    let opened_account_ids = Arc::new(Mutex::new(Vec::new()));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
        session: Arc::clone(&session_b),
        fallback_removals: Arc::new(AtomicBool::new(false)),
        session_open_attempts,
        opened_account_ids: Arc::clone(&opened_account_ids),
    });
    let workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let (view, context) =
        cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));
    context.run_until_parked();
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(profile_a);
            view.account_session = Some(Arc::clone(&session_a));
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "sign-out");

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).status,
        AccountStatus::Failed {
            error: AccountOperationError::SavedAccountIdentityUnavailable,
            saved_credential_may_exist: true,
        }
    )));
    assert!(context.cx.update(|cx| {
        view.read(cx)
            .account_session
            .as_ref()
            .is_some_and(|retained| Arc::ptr_eq(retained, &session_a))
    }));

    *provider
        .saved_account_id
        .lock()
        .expect("synthetic saved account ID mutex is healthy") = Some(account_b_id.clone());
    *provider
        .restore_profile
        .lock()
        .expect("synthetic restored profile mutex is healthy") = Some(profile_b.clone());
    click(context, "retry-account-restore");

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).status,
        AccountStatus::Connected(profile) if profile.id() == &account_b_id
    )));
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).account_session.is_none())
    );

    click(context, "open-mail");

    assert_eq!(
        *opened_account_ids
            .lock()
            .expect("synthetic opened-account ID mutex is healthy"),
        vec![account_b_id.clone()]
    );
    assert!(context.cx.update(|cx| {
        view.read(cx)
            .account_session
            .as_ref()
            .is_some_and(|retained| Arc::ptr_eq(retained, &session_b))
    }));

    click(context, "mail-account-navigation");
    click(context, "sign-out");

    let retry_state_is_bound_to_account_b = context.cx.update(|cx| {
        let view = view.read(cx);
        matches!(
            &view.status,
            AccountStatus::Connected(profile) if profile.id() == &account_b_id
        ) && view
            .account_session
            .as_ref()
            .is_some_and(|retained| Arc::ptr_eq(retained, &session_b))
    });
    assert!(retry_state_is_bound_to_account_b);
    assert!(
        session_a_removals
            .lock()
            .expect("synthetic account A removal log mutex is healthy")
            .is_empty()
    );
    assert_eq!(
        *session_b_removals
            .lock()
            .expect("synthetic account B removal log mutex is healthy"),
        vec![account_b_id.clone()]
    );

    click(context, "sign-out");

    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).status, AccountStatus::Disconnected))
    );
    assert!(signed_out.load(Ordering::SeqCst));
    assert_eq!(
        *session_b_removals
            .lock()
            .expect("synthetic account B removal log mutex is healthy"),
        vec![account_b_id.clone(), account_b_id]
    );
}

#[gpui_kit::test]
fn sign_out_after_calendar_return_reuses_the_retained_session(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = Arc::clone(&provider.signed_out);
    *provider
        .saved_account_id
        .lock()
        .expect("synthetic saved account ID mutex is healthy") = Some(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
    );
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let removal_attempts = Arc::new(Mutex::new(Vec::new()));
    let fallback_removals = Arc::new(AtomicBool::new(false));
    let session: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid"),
        removal_attempts: Arc::clone(&removal_attempts),
    });
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
        session: Arc::clone(&session),
        fallback_removals: Arc::clone(&fallback_removals),
        session_open_attempts: Arc::new(AtomicUsize::new(0)),
        opened_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));
    context.run_until_parked();
    let profile = AccountProfile::try_new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
    )
    .expect("synthetic account profile is valid");
    let account_id = profile.id().clone();
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(profile);
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "open-calendar");
    let calendar_open = context
        .cx
        .update(|cx| matches!(&view.read(cx).content, AccountContent::Calendar { .. }));
    assert!(calendar_open);

    click(context, "calendar-back");
    let returned_to_account = context
        .cx
        .update(|cx| matches!(&view.read(cx).content, AccountContent::Account));
    assert!(returned_to_account);

    let session_retained_after_return = context.cx.update(|cx| {
        view.read(cx)
            .account_session
            .as_ref()
            .is_some_and(|retained| Arc::ptr_eq(retained, &session))
    });
    assert!(session_retained_after_return);

    click(context, "sign-out");

    let retry_state_kept = context.cx.update(|cx| {
        let account_view = view.read(cx);
        matches!(
            &account_view.status,
            AccountStatus::Connected(profile) if profile.id() == &account_id
        ) && account_view
            .account_session
            .as_ref()
            .is_some_and(|retained| Arc::ptr_eq(retained, &session))
    });
    assert!(retry_state_kept);
    assert_eq!(
        *removal_attempts
            .lock()
            .expect("synthetic session removal log mutex is healthy"),
        vec![account_id.clone()]
    );
    assert!(!fallback_removals.load(Ordering::SeqCst));

    click(context, "sign-out");

    let state_cleared = context.cx.update(|cx| {
        let account_view = view.read(cx);
        matches!(&account_view.status, AccountStatus::Disconnected)
            && account_view.account_session.is_none()
    });
    assert!(state_cleared);
    assert!(signed_out.load(Ordering::SeqCst));
    assert_eq!(
        *removal_attempts
            .lock()
            .expect("synthetic session removal log mutex is healthy"),
        vec![account_id.clone(), account_id]
    );
    assert!(!fallback_removals.load(Ordering::SeqCst));
}

#[gpui_kit::test]
fn mail_and_calendar_permission_reviews_work_after_content_opens(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let session_open_attempts = Arc::new(AtomicUsize::new(0));
    let session: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid"),
        removal_attempts: Arc::new(Mutex::new(Vec::new())),
    });
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
        session: Arc::clone(&session),
        fallback_removals: Arc::new(AtomicBool::new(false)),
        session_open_attempts: Arc::clone(&session_open_attempts),
        opened_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));
    context.run_until_parked();
    let profile = AccountProfile::try_new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
    )
    .expect("synthetic account profile is valid");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(profile);
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "open-mail");
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).content, AccountContent::Mail { .. }))
    );

    let mail_view = context.cx.update(|cx| match &view.read(cx).content {
        AccountContent::Mail { view, .. } => view.clone(),
        _ => panic!("mail view is open"),
    });
    context.cx.update(|cx| {
        mail_view.update(cx, |_, cx| {
            cx.emit(crate::mail_view::MailViewEvent::ReviewMailWritePermission);
        });
    });
    context.run_until_parked();
    draw_account_view(context);
    assert!(context.cx.update(|cx| {
        let account_view = view.read(cx);
        matches!(&account_view.content, AccountContent::Account)
            && matches!(
                account_view.pending_permission_review,
                Some(AccountPermissionRequest::MailReadWrite)
            )
    }));
    click(context, "cancel-permission-review");
    assert!(
        context
            .cx
            .update(|cx| { view.read(cx).pending_permission_review.is_none() })
    );

    click(context, "open-mail");
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).content, AccountContent::Mail { .. }))
    );
    click(context, "mail-account-navigation");
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).content, AccountContent::Account))
    );
    assert!(context.cx.update(|cx| {
        view.read(cx)
            .account_session
            .as_ref()
            .is_some_and(|retained| Arc::ptr_eq(retained, &session))
    }));

    click(context, "open-calendar");
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).content, AccountContent::Calendar { .. }))
    );
    let calendar_view = context.cx.update(|cx| match &view.read(cx).content {
        AccountContent::Calendar { view, .. } => view.clone(),
        _ => panic!("calendar view is open"),
    });
    context.cx.update(|cx| {
        calendar_view.update(cx, |_, cx| {
            cx.emit(crate::calendar_view::CalendarViewEvent::ReviewCalendarWritePermission);
        });
    });
    context.run_until_parked();
    draw_account_view(context);
    assert!(context.cx.update(|cx| {
        let account_view = view.read(cx);
        matches!(&account_view.content, AccountContent::Account)
            && matches!(
                account_view.pending_permission_review,
                Some(AccountPermissionRequest::CalendarReadWrite)
            )
    }));
    click(context, "cancel-permission-review");
    assert!(
        context
            .cx
            .update(|cx| { view.read(cx).pending_permission_review.is_none() })
    );
    assert_eq!(session_open_attempts.load(Ordering::SeqCst), 1);
}

#[gpui_kit::test]
fn connection_cancel_resolves_before_and_during_credential_commit(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    provider.pause_connection.store(true, Ordering::SeqCst);
    let workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let mut account_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| AccountView::new(Ok(workflow), None, cx));
        account_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = account_view.expect("account view is created in its window");

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-account");
    click(context, "cancel-sign-in");

    let cancelled = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::Failed {
                error: AccountOperationError::SignInCancelled,
                ..
            }
        )
    });
    assert!(cancelled);

    provider.pause_connection.store(false, Ordering::SeqCst);
    let (commit_started, finish_commit) = provider.pause_next_connection_commit();
    click(context, "connect-account");
    commit_started
        .recv_timeout(Duration::from_secs(1))
        .expect("retry reaches the credential commit");
    click(context, "cancel-sign-in");

    let cancelling = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::CancellingConnect { .. }
        )
    });
    assert!(cancelling);
    assert!(context.debug_bounds("account-operation-progress").is_some());

    finish_commit
        .send(())
        .expect("synthetic credential commit is released");
    context.run_until_parked();
    draw_account_view(context);

    let connected = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::Connected(profile) if profile.label() == "Ada Example"
        )
    });
    assert!(connected);
}

#[gpui_kit::test]
fn permission_cancel_resolves_before_and_during_credential_commit(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    provider
        .pause_permission_update
        .store(true, Ordering::SeqCst);
    let workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let mut account_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| AccountView::new(Ok(workflow), None, cx));
        account_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = account_view.expect("account view is created in its window");

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-account");
    click(context, "review-app-permissions");
    click(context, "continue-permission-review");
    click(context, "cancel-permission-update");

    let cancelled_with_profile = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::ReauthorizationFailed {
                profile,
                error: AccountOperationError::SignInCancelled,
            } if profile.label() == "Ada Example"
        )
    });
    assert!(cancelled_with_profile);

    provider
        .pause_permission_update
        .store(false, Ordering::SeqCst);
    let (commit_started, finish_commit) = provider.pause_next_permission_commit();
    click(context, "continue-permission-review");
    commit_started
        .recv_timeout(Duration::from_secs(1))
        .expect("permission retry reaches the credential commit");
    click(context, "cancel-permission-update");

    let cancelling_with_profile = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::CancellingReauthorization { profile }
                if profile.label() == "Ada Example"
        )
    });
    assert!(cancelling_with_profile);
    assert!(context.debug_bounds("account-operation-progress").is_some());

    finish_commit
        .send(())
        .expect("synthetic credential commit is released");
    context.run_until_parked();
    draw_account_view(context);

    let connected = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::Connected(profile) if profile.label() == "Ada Example"
        )
    });
    assert!(connected);
}

#[test]
fn reauthorization_mismatch_copy_confirms_saved_sign_in_was_preserved() {
    assert_eq!(
        AccountStatus::reauthorization_error_message(AccountOperationError::AccountMismatch),
        "Microsoft returned a different account. Your saved sign-in was not changed."
    );
}

#[gpui_kit::test]
fn mail_write_scope_requires_review_before_the_browser_flow(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let permission_request = Arc::clone(&provider.last_permission_request);
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let (view, context) = cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), None, cx));

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-account");
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).status, AccountStatus::Connected(_)))
    );
    context.simulate_resize(size(px(800.), px(600.)));
    draw_account_view(context);
    let viewport = context.update(|window, _| window.bounds());
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.prepare_permission_review(AccountPermissionRequest::MailReadWrite, cx);
        });
    });
    draw_account_view(context);

    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).pending_permission_review),
        Some(AccountPermissionRequest::MailReadWrite)
    );
    let can_continue = context.cx.update(|cx| {
        let view = view.read(cx);
        view.task.is_none() && view.content.opening_kind().is_none()
    });
    assert!(can_continue);
    let disclosure = super::permission_review_disclosure(AccountPermissionRequest::MailReadWrite);
    assert!(disclosure.contains("create, read, update, and delete mail"));
    assert!(disclosure.contains("move messages between folders"));
    assert!(disclosure.contains("delete selected messages after confirmation"));
    assert!(disclosure.contains("does not request Mail.Send"));
    assert_eq!(
        *permission_request
            .lock()
            .expect("synthetic permission request mutex is not poisoned"),
        None
    );
    assert!(context.debug_bounds("continue-permission-review").is_some());
    assert!(
        context
            .debug_bounds("continue-permission-review")
            .expect("continue action is rendered")
            .bottom()
            > viewport.bottom()
    );

    scroll_account_action_into_view(context, "continue-permission-review");
    let continue_bounds = context
        .debug_bounds("continue-permission-review")
        .expect("continue action remains rendered after scrolling");
    assert!(continue_bounds.top() >= viewport.top());
    assert!(continue_bounds.bottom() <= viewport.bottom());

    click(context, "continue-permission-review");

    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).status, AccountStatus::Connected(_)))
    );

    assert_eq!(
        *permission_request
            .lock()
            .expect("synthetic permission request mutex is not poisoned"),
        Some(AccountPermissionRequest::MailReadWrite)
    );
}

#[gpui_kit::test]
fn missing_client_configuration_disables_account_actions(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, context) = cx.add_window_view(|_, cx| {
        AccountView::<TestProvider>::new(Err(SetupIssue::MissingClientId), None, cx)
    });

    draw_account_view(context);

    assert!(context.debug_bounds("connect-account").is_none());
    assert_eq!(
        context.cx.update(|cx| view.read(cx).setup_issue),
        Some(SetupIssue::MissingClientId)
    );
}
