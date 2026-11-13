use std::sync::Arc;

use super::state::{
    AccountSignOutError, AccountStatus, EwsDiscoveryState, ExchangeEwsConnectionForm,
    SavedCredentialState,
};
use super::{AccountView, AccountViewEvent, SignOutSessionDisposition};
use einfach_mail_application::{
    AccountCacheRemovalError, AccountConnectionRequest, AccountOperationCancellation,
    AccountOperationError, AccountPermissionRequest, AccountProvider, AccountSession,
    AccountSessionFactory, AccountWorkflow, ConnectedAccount, ExchangeEwsDiscoveryConfirmation,
    ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult,
};
use gpui_kit::{AsyncApp, Context, Task, Window};

impl<P: AccountProvider + 'static> AccountView<P> {
    pub(super) fn start_restore(&mut self, cx: &mut Context<Self>) {
        let Ok(services) = &self.services else {
            return;
        };
        let workflow = Arc::clone(&services.workflow);
        self.status = AccountStatus::Restoring;
        self.cache_removal_error = None;
        cx.emit(AccountViewEvent::RestoreStarted);
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let restore_result = workflow.restore_account().await;
            let _ = this.update(cx, |view, cx| {
                view.task = None;
                view.status = match restore_result {
                    Ok(Some(account)) => AccountStatus::Connected(account),
                    Ok(None) => AccountStatus::Disconnected,
                    Err(error) => AccountStatus::Failed {
                        error,
                        saved_credential_state: SavedCredentialState::MayExist,
                    },
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn connect(&mut self, cx: &mut Context<Self>) {
        self.begin_connect(AccountConnectionRequest::Microsoft365, cx);
    }

    pub(super) fn begin_exchange_ews_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.task.is_some()
            || !matches!(
                &self.status,
                AccountStatus::Disconnected
                    | AccountStatus::Failed {
                        saved_credential_state: SavedCredentialState::Absent,
                        ..
                    }
            )
        {
            return;
        }
        self.exchange_ews_form = Some(ExchangeEwsConnectionForm::new(window, cx));
        cx.notify();
    }

    /// Reports whether the form is open, so the preview can stop waiting once it is.
    #[cfg(feature = "ui-preview")]
    pub(crate) fn open_exchange_form_for_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.begin_exchange_ews_form(window, cx);
        self.exchange_ews_form.is_some()
    }

    pub(super) fn cancel_exchange_ews_form(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, AccountStatus::Connecting { .. }) {
            self.cancel_connect(cx);
        } else if !matches!(self.status, AccountStatus::CancellingConnect) {
            let confirmation_pending = self.pending_ews_discovery_confirmation().is_some();
            let discovery_busy = self
                .exchange_ews_form
                .as_ref()
                .is_some_and(|form| form.discovery.is_busy());
            if confirmation_pending {
                self.cancel_exchange_ews_discovery(cx);
            } else if discovery_busy {
                self.task = None;
            }
            self.exchange_ews_form = None;
            cx.notify();
        }
    }

    pub(super) fn connect_exchange_ews(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.exchange_ews_form else {
            return;
        };
        let request = form.request(cx);
        self.begin_connect(request, cx);
    }

    pub(super) fn discover_exchange_ews_endpoint(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.task.is_some() || !self.ews_discovery_can_start() {
            return;
        }
        let Some(mailbox_address) = self.ews_mailbox_address(cx) else {
            return;
        };
        self.start_ews_discovery(
            ExchangeEwsDiscoveryRequest::Start { mailbox_address },
            window,
            cx,
        );
    }

    fn ews_discovery_can_start(&self) -> bool {
        self.exchange_ews_form
            .as_ref()
            .is_some_and(|form| form.discovery.allows_start())
    }

    fn ews_mailbox_address(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let Some(form) = &mut self.exchange_ews_form else {
            return None;
        };
        let mailbox_address = form.mailbox_address.read(cx).value().to_string();
        if mailbox_address.trim().is_empty() {
            form.discovery = EwsDiscoveryState::Failed;
            cx.notify();
            return None;
        }
        Some(mailbox_address)
    }

    pub(super) fn continue_exchange_ews_discovery(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(confirmation) = self.pending_ews_discovery_confirmation().cloned() else {
            return;
        };
        self.start_ews_discovery(
            ExchangeEwsDiscoveryRequest::Continue(confirmation),
            window,
            cx,
        );
    }

    pub(super) fn cancel_exchange_ews_discovery(&mut self, cx: &mut Context<Self>) {
        let Some(confirmation) = self.pending_ews_discovery_confirmation().cloned() else {
            return;
        };
        let request = ExchangeEwsDiscoveryRequest::Cancel(confirmation);
        let Ok(services) = &self.services else {
            self.set_ews_discovery_idle(cx);
            return;
        };
        let workflow = Arc::clone(&services.workflow);
        self.set_ews_discovery_state(EwsDiscoveryState::Cancelling);
        self.schedule_ews_discovery_cancellation(workflow, request, cx);
        cx.notify();
    }

    fn pending_ews_discovery_confirmation(&self) -> Option<&ExchangeEwsDiscoveryConfirmation> {
        let form = self.exchange_ews_form.as_ref()?;
        let EwsDiscoveryState::Confirmation(confirmation) = &form.discovery else {
            return None;
        };
        Some(confirmation)
    }

    fn set_ews_discovery_state(&mut self, state: EwsDiscoveryState) {
        if let Some(form) = &mut self.exchange_ews_form {
            form.discovery = state;
        }
    }

    fn set_ews_discovery_idle(&mut self, cx: &mut Context<Self>) {
        self.set_ews_discovery_state(EwsDiscoveryState::Idle);
        cx.notify();
    }

    fn schedule_ews_discovery_cancellation(
        &mut self,
        workflow: Arc<AccountWorkflow<P>>,
        request: ExchangeEwsDiscoveryRequest,
        cx: &mut Context<Self>,
    ) {
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let _ = workflow.discover_exchange_ews_endpoint(request).await;
            let _ = this.update(cx, |view, cx| view.finish_ews_discovery_cancellation(cx));
        }));
    }

    fn finish_ews_discovery_cancellation(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.set_ews_discovery_idle(cx);
    }

    fn start_ews_discovery(
        &mut self,
        request: ExchangeEwsDiscoveryRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(services) = &self.services else {
            return;
        };
        let workflow = Arc::clone(&services.workflow);
        let Some(form) = &mut self.exchange_ews_form else {
            return;
        };
        form.discovery = EwsDiscoveryState::Searching;
        self.schedule_ews_discovery(workflow, request, window, cx);
    }

    fn schedule_ews_discovery(
        &mut self,
        workflow: Arc<AccountWorkflow<P>>,
        request: ExchangeEwsDiscoveryRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let discovery_result = workflow.discover_exchange_ews_endpoint(request).await;
            let _ = this.update_in(cx, |view, window, cx| {
                view.task = None;
                view.apply_ews_discovery_result(discovery_result, window, cx);
            });
        }));
        cx.notify();
    }

    fn apply_ews_discovery_result(
        &mut self,
        discovery_result: Result<ExchangeEwsDiscoveryResult, AccountOperationError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exchange_ews_form.is_none() {
            return;
        }
        match discovery_result {
            Ok(ExchangeEwsDiscoveryResult::Endpoint(endpoint)) => {
                self.apply_discovered_ews_endpoint(endpoint, window, cx);
            }
            other => self.set_ews_discovery_state(EwsDiscoveryState::from_result(other)),
        }
        cx.notify();
    }

    fn apply_discovered_ews_endpoint(
        &mut self,
        endpoint: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &mut self.exchange_ews_form else {
            return;
        };
        form.endpoint
            .update(cx, |input, cx| input.set_value(endpoint, window, cx));
        form.discovery = EwsDiscoveryState::Found;
    }

    fn begin_connect(&mut self, request: AccountConnectionRequest, cx: &mut Context<Self>) {
        let Ok(services) = &self.services else {
            return;
        };
        let workflow = Arc::clone(&services.workflow);
        if !matches!(
            &self.status,
            AccountStatus::Disconnected
                | AccountStatus::Failed {
                    saved_credential_state: SavedCredentialState::Absent,
                    ..
                }
        ) {
            return;
        }
        let cancellation = AccountOperationCancellation::new();
        self.status = AccountStatus::Connecting {
            cancellation: cancellation.clone(),
        };
        self.task = Some(Self::connect_task(workflow, request, cancellation, cx));
        cx.notify();
    }

    fn connect_task(
        workflow: Arc<AccountWorkflow<P>>,
        request: AccountConnectionRequest,
        cancellation: AccountOperationCancellation,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let connection_result = workflow
                .connect_account_with_request_and_cancellation(request, cancellation)
                .await;
            let _ = this.update(cx, |view, cx| view.finish_connect(connection_result, cx));
        })
    }

    fn finish_connect(
        &mut self,
        connection_result: Result<ConnectedAccount, AccountOperationError>,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        self.status = match connection_result {
            Ok(account) => {
                self.exchange_ews_form = None;
                AccountStatus::Connected(account)
            }
            Err(error) => AccountStatus::Failed {
                saved_credential_state: SavedCredentialState::after_connection_error(error),
                error,
            },
        };
        cx.notify();
    }

    pub(super) fn cancel_connect(&mut self, cx: &mut Context<Self>) {
        let AccountStatus::Connecting { cancellation } = &self.status else {
            return;
        };
        cancellation.cancel();
        self.status = AccountStatus::CancellingConnect;
        cx.notify();
    }

    pub(crate) fn prepare_permission_review(
        &mut self,
        request: AccountPermissionRequest,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_is_opening || self.status.connected_profile().is_none() {
            return;
        }
        if request == AccountPermissionRequest::MailReadWrite
            && !self
                .status
                .connected_profile()
                .is_some_and(|profile| profile.capabilities().can_review_mail_write_permission())
        {
            return;
        }
        self.pending_permission_review = Some(request);
        cx.notify();
    }

    pub(super) fn continue_permission_review(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.pending_permission_review.take() else {
            return;
        };
        self.reauthorize(request, cx);
    }

    pub(super) fn cancel_permission_review(&mut self, cx: &mut Context<Self>) {
        self.pending_permission_review = None;
        cx.notify();
    }

    fn reauthorize(&mut self, request: AccountPermissionRequest, cx: &mut Context<Self>) {
        let Ok(services) = &self.services else {
            return;
        };
        let workflow = Arc::clone(&services.workflow);
        let Some(account) = self.status.connected_account().cloned() else {
            return;
        };
        if request == AccountPermissionRequest::MailReadWrite
            && !account
                .profile()
                .capabilities()
                .can_review_mail_write_permission()
        {
            return;
        }
        let cancellation = AccountOperationCancellation::new();
        self.pending_permission_review = None;
        self.status = AccountStatus::Reauthorizing {
            account: account.clone(),
            cancellation: cancellation.clone(),
        };
        self.task = Some(Self::reauthorize_task(
            workflow,
            account,
            request,
            cancellation,
            cx,
        ));
        cx.notify();
    }

    fn reauthorize_task(
        workflow: Arc<AccountWorkflow<P>>,
        account: ConnectedAccount,
        request: AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let reauthorization_result = workflow
                .reauthorize_account_with_cancellation(
                    account.profile().id(),
                    request,
                    cancellation,
                )
                .await;
            let _ = this.update(cx, |view, cx| {
                view.task = None;
                if reauthorization_result.is_err() {
                    view.pending_permission_review = Some(request);
                }
                view.status = match reauthorization_result {
                    Ok(account) => AccountStatus::Connected(account),
                    Err(error) => AccountStatus::ReauthorizationFailed { account, error },
                };
                cx.notify();
            });
        })
    }

    pub(super) fn cancel_reauthorization(&mut self, cx: &mut Context<Self>) {
        let AccountStatus::Reauthorizing {
            account,
            cancellation,
        } = &self.status
        else {
            return;
        };
        cancellation.cancel();
        self.status = AccountStatus::CancellingReauthorization {
            account: account.clone(),
        };
        cx.notify();
    }

    pub(crate) fn request_sign_out(&mut self, cx: &mut Context<Self>) {
        cx.emit(AccountViewEvent::SignOutRequested);
    }

    pub(crate) fn sign_out(
        &mut self,
        account_session: Option<Arc<dyn AccountSession>>,
        cx: &mut Context<Self>,
    ) {
        let Ok(services) = &self.services else {
            return;
        };
        if !self.can_sign_out() {
            return;
        }

        let workflow = Arc::clone(&services.workflow);
        let account_session_factory = Arc::clone(&services.account_session_factory);
        let previous_status = self.status.clone();
        let account = previous_status.connected_account().cloned();
        self.cache_removal_error = None;
        self.status = AccountStatus::SigningOut { account };
        self.task = Some(Self::sign_out_task(
            workflow,
            previous_status,
            account_session,
            account_session_factory,
            cx,
        ));
        cx.notify();
    }

    fn can_sign_out(&self) -> bool {
        self.status.saved_credential_state() != SavedCredentialState::Absent
            && !self.workspace_is_opening
            && !matches!(
                &self.status,
                AccountStatus::Connecting { .. }
                    | AccountStatus::CancellingConnect
                    | AccountStatus::Reauthorizing { .. }
                    | AccountStatus::CancellingReauthorization { .. }
                    | AccountStatus::SigningOut { .. }
            )
    }

    fn sign_out_task(
        workflow: Arc<AccountWorkflow<P>>,
        previous_status: AccountStatus,
        account_session: Option<Arc<dyn AccountSession>>,
        account_session_factory: Arc<dyn AccountSessionFactory>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = cx
                .background_executor()
                .spawn(remove_verified_account_cache_and_sign_out(
                    workflow,
                    account_session,
                    account_session_factory,
                ))
                .await;
            let _ = this.update(cx, |view, cx| {
                view.finish_sign_out(previous_status, result, cx);
            });
        })
    }

    fn finish_sign_out(
        &mut self,
        previous_status: AccountStatus,
        result: Result<(), AccountSignOutError>,
        cx: &mut Context<Self>,
    ) {
        let session_disposition = sign_out_session_disposition(&result);
        match result {
            Ok(()) => self.status = AccountStatus::Disconnected,
            Err(AccountSignOutError::Identity(error)) => {
                self.status = AccountStatus::Failed {
                    error,
                    saved_credential_state: SavedCredentialState::MayExist,
                };
            }
            Err(AccountSignOutError::CacheRemoval(error)) => {
                self.status = previous_status;
                self.cache_removal_error = Some(error);
            }
            Err(AccountSignOutError::CredentialRemoval(error)) => {
                self.status = AccountStatus::SignOutFailed {
                    account: previous_status.connected_account().cloned(),
                    error,
                };
            }
        }
        self.task = None;
        cx.emit(AccountViewEvent::SignOutFinished(session_disposition));
        cx.notify();
    }
}

fn sign_out_session_disposition(
    result: &Result<(), AccountSignOutError>,
) -> SignOutSessionDisposition {
    match result {
        Ok(()) => SignOutSessionDisposition::Release,
        Err(_) => SignOutSessionDisposition::Retain,
    }
}

async fn remove_account_cache(
    account_session: Option<Arc<dyn AccountSession>>,
    account_session_factory: Arc<dyn AccountSessionFactory>,
    account_id: einfach_mail_domain::AccountId,
) -> Result<(), AccountCacheRemovalError> {
    match account_session {
        Some(account_session) => account_session.remove_account_cache().await,
        None => {
            account_session_factory
                .remove_account_cache(&account_id)
                .await
        }
    }
}

async fn remove_verified_account_cache_and_sign_out<P: AccountProvider>(
    workflow: Arc<AccountWorkflow<P>>,
    account_session: Option<Arc<dyn AccountSession>>,
    account_session_factory: Arc<dyn AccountSessionFactory>,
) -> Result<(), AccountSignOutError> {
    let account_id = workflow
        .saved_credential_account_id()
        .await
        .map_err(AccountSignOutError::Identity)?
        .ok_or(AccountSignOutError::Identity(
            AccountOperationError::SavedAccountIdentityUnavailable,
        ))?;
    remove_account_cache(account_session, account_session_factory, account_id)
        .await
        .map_err(AccountSignOutError::CacheRemoval)?;
    workflow
        .sign_out()
        .await
        .map_err(AccountSignOutError::CredentialRemoval)
}
