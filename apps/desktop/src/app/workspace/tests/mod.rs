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

use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{
    AppContext as _, Modifiers, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point,
    px, size,
};

use super::WorkspaceSection;
use super::{AccountContent, AccountServices, AccountStatus, AccountView, SetupIssue};
use einfach_mail_application::{
    AccountCacheOpenError, AccountCacheRemovalError, AccountConnectionRequest,
    AccountOperationCancellation, AccountOperationError, AccountPermissionRequest, AccountProvider,
    AccountProviderKind, AccountSession, AccountSessionFactory, AccountSessionFuture,
    AccountWorkflow, CalendarCacheError, CalendarEventCreationOutcome,
    CalendarEventDeletionOutcome, CalendarEventUpdateOutcome, CalendarSession, CalendarSnapshot,
    CalendarSyncReport, CalendarWorkflowError, ConnectedAccount, ExchangeEwsDiscoveryConfirmation,
    ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult, FollowUpFlagUpdateOutcome,
    MailCacheError, MailSendOutcome, MailSendResolutionError, MailSendWorkflowError, MailSession,
    MailSyncProgress, MailSyncReport, MailSyncState, MailWorkflowError, MessageMoveOutcome,
    ReadStateUpdateOutcome, WellKnownFolderMoveOutcome, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountCapabilities, AccountId, AccountProfile, CalendarAccess, CalendarEventCreation,
    CalendarEventUpdate, CalendarOccurrence, CalendarViewReference, EditableCalendarEvent,
    FolderReference, FollowUpFlagStatus, MailAction, MailCapabilities, MailDraft, MailFolder,
    MailMessage, MailReplyDraft, MailSendOperationId, MessageId, PendingMailSend, ReadState,
};

fn mail_only_profile() -> AccountProfile {
    AccountProfile::try_new_with_capabilities(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::read_only_mail(),
    )
    .expect("synthetic mail-only account profile is valid")
}

fn connected_account(profile: AccountProfile) -> ConnectedAccount {
    ConnectedAccount::new(profile, AccountProviderKind::MicrosoftGraph)
}

#[derive(Clone, Default)]
struct TestProvider {
    signed_out: Arc<AtomicBool>,
    fail_next_sign_out: Arc<AtomicBool>,
    pause_connection: Arc<AtomicBool>,
    pause_permission_update: Arc<AtomicBool>,
    restore_error: Arc<Mutex<Option<AccountOperationError>>>,
    restore_profile: Arc<Mutex<Option<AccountProfile>>>,
    connect_error: Arc<Mutex<Option<AccountOperationError>>>,
    ews_connect_requests: Arc<AtomicUsize>,
    ews_input_valid: Arc<AtomicBool>,
    ews_discovery_requests: Arc<AtomicUsize>,
    ews_discovery_input_valid: Arc<AtomicBool>,
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

fn unused_account_session_factory() -> Arc<dyn AccountSessionFactory> {
    Arc::new(TestAccountSessionFactory {
        session: Arc::new(TestAccountSession {
            account_id: AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is valid"),
            removal_attempts: Arc::new(Mutex::new(Vec::new())),
        }),
        fallback_removals: Arc::new(AtomicBool::new(false)),
        session_open_attempts: Arc::new(AtomicUsize::new(0)),
        opened_account_ids: Arc::new(Mutex::new(Vec::new())),
    })
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
    ) -> impl Future<Output = Result<Option<ConnectedAccount>, AccountOperationError>> + Send + '_
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
        ready(restore_error.map_or(Ok(restore_profile.map(connected_account)), Err))
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
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
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
                commit_gate
                    .started
                    .send(())
                    .expect("the account view test is waiting at credential commit");
                commit_gate
                    .finish
                    .await
                    .map_err(|_| AccountOperationError::ServiceUnavailable)?;
            }
            *saved_account_id
                .lock()
                .expect("synthetic saved account ID mutex is healthy") = Some(account_id);
            signed_out.store(false, Ordering::SeqCst);
            Ok(connected_account(profile))
        }
    }

    fn connect_account_with_request(
        &self,
        request: AccountConnectionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
        let ews_connect_requests = Arc::clone(&self.ews_connect_requests);
        let ews_input_valid = Arc::clone(&self.ews_input_valid);
        let saved_account_id = Arc::clone(&self.saved_account_id);
        let signed_out = Arc::clone(&self.signed_out);
        async move {
            match request {
                AccountConnectionRequest::Microsoft365 => self.connect_account(cancellation).await,
                AccountConnectionRequest::ExchangeEws(input) => {
                    let valid = input.endpoint()
                        == "https://exchange.example.test/EWS/Exchange.asmx"
                        && input.mailbox_address() == "ada@example.test"
                        && input.username() == "EXAMPLE\\ada"
                        && input.expose_password() == "synthetic-only";
                    ews_connect_requests.fetch_add(1, Ordering::SeqCst);
                    ews_input_valid.store(valid, Ordering::SeqCst);
                    if !valid {
                        return Err(AccountOperationError::InvalidConnectionDetails);
                    }
                    cancellation.begin_credential_commit()?;
                    let profile = AccountProfile::try_new_with_capabilities(
                        AccountId::try_from("synthetic-ews-account".to_owned())
                            .expect("synthetic EWS account ID is valid"),
                        Some("Ada Example".to_owned()),
                        Some("ada@example.test".to_owned()),
                        Some("EXAMPLE\\ada".to_owned()),
                        AccountCapabilities::new(
                            MailCapabilities::from_actions([MailAction::SetReadState]),
                            CalendarAccess::ReadOnly,
                        ),
                    )
                    .expect("synthetic EWS account profile has an identity");
                    *saved_account_id
                        .lock()
                        .expect("synthetic saved account ID mutex is healthy") =
                        Some(profile.id().clone());
                    signed_out.store(false, Ordering::SeqCst);
                    Ok(ConnectedAccount::new(
                        profile,
                        AccountProviderKind::ExchangeEws,
                    ))
                }
            }
        }
    }

    fn discover_exchange_ews_endpoint(
        &self,
        request: ExchangeEwsDiscoveryRequest,
    ) -> impl Future<Output = Result<ExchangeEwsDiscoveryResult, AccountOperationError>> + Send + '_
    {
        match request {
            ExchangeEwsDiscoveryRequest::Start { mailbox_address } => {
                self.ews_discovery_requests.fetch_add(1, Ordering::SeqCst);
                self.ews_discovery_input_valid
                    .store(mailbox_address == "ada@example.test", Ordering::SeqCst);
                ready(Ok(ExchangeEwsDiscoveryResult::ConfirmationRequired(
                    ExchangeEwsDiscoveryConfirmation::new(
                        41,
                        mailbox_address,
                        "https://redirect.example.test".to_owned(),
                    ),
                )))
            }
            ExchangeEwsDiscoveryRequest::Continue(confirmation)
                if confirmation.challenge() == 41 =>
            {
                ready(Ok(ExchangeEwsDiscoveryResult::Endpoint(
                    "https://exchange.example.test/EWS/Exchange.asmx".to_owned(),
                )))
            }
            ExchangeEwsDiscoveryRequest::Cancel(_) => {
                ready(Ok(ExchangeEwsDiscoveryResult::Cancelled))
            }
            _ => ready(Err(AccountOperationError::InvalidConnectionDetails)),
        }
    }

    fn reauthorize_account(
        &self,
        expected_account_id: &AccountId,
        request: AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
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
                commit_gate
                    .started
                    .send(())
                    .expect("the account view test is waiting at permission commit");
                commit_gate
                    .finish
                    .await
                    .map_err(|_| AccountOperationError::ServiceUnavailable)?;
            }
            Ok(connected_account(profile))
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

mod account_setup;
mod account_state;
mod navigation;
mod permission_review;
mod session_lifecycle;
mod status_copy;
