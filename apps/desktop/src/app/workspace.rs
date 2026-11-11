use std::sync::Arc;

use crate::features::{
    CalendarView, CalendarViewEvent, MailFolderNavigation, MailPermissionRequester, MailView,
    MailViewEvent,
};
use einfach_mail_application::{
    AccountCacheOpenError, AccountCacheRemovalError, AccountConnectionRequest,
    AccountOperationCancellation, AccountOperationError, AccountPermissionRequest, AccountProvider,
    AccountProviderKind, AccountSession, AccountSessionFactory, AccountWorkflow, ConnectedAccount,
    ExchangeEwsConnectionInput, ExchangeEwsDiscoveryConfirmation, ExchangeEwsDiscoveryRequest,
    ExchangeEwsDiscoveryResult,
};
use einfach_mail_domain::{AccountProfile, CalendarAccess, MailAction, MailCapabilities};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Icon, IconName, Selectable, StyledExt,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    input::{Input, InputState},
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    App, AsyncApp, Context, Div, Entity, FocusHandle, IntoElement, Render, Role, Stateful,
    Subscription, Task, Window, div, prelude::*, rems,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SetupIssue {
    MissingClientId,
    InvalidClientId,
    MissingCallbackPort,
    InvalidCallbackPort,
    ExchangeSettingsUnavailable,
}

impl SetupIssue {
    fn message(self) -> &'static str {
        match self {
            Self::MissingClientId => {
                "Account sign-in is not configured. Set EINFACH_MAIL_ENTRA_CLIENT_ID to a Microsoft public client app ID."
            }
            Self::InvalidClientId => {
                "The Microsoft public client app ID is invalid. Check EINFACH_MAIL_ENTRA_CLIENT_ID."
            }
            Self::MissingCallbackPort => {
                "Sign-in needs a registered loopback port. Set EINFACH_MAIL_ENTRA_CALLBACK_PORT to the port registered for http://127.0.0.1."
            }
            Self::InvalidCallbackPort => {
                "The OAuth callback port must be a nonzero TCP port registered for http://127.0.0.1."
            }
            Self::ExchangeSettingsUnavailable => {
                "Exchange account settings are unavailable. Check the Linux configuration directory and try again."
            }
        }
    }
}

pub(super) struct AccountServices<P> {
    pub(super) workflow: Arc<AccountWorkflow<P>>,
    pub(super) account_session_factory: Arc<dyn AccountSessionFactory>,
    microsoft365_setup_issue: Option<SetupIssue>,
}

impl<P> AccountServices<P> {
    pub(super) fn new(
        workflow: Arc<AccountWorkflow<P>>,
        account_session_factory: Arc<dyn AccountSessionFactory>,
    ) -> Self {
        Self {
            workflow,
            account_session_factory,
            microsoft365_setup_issue: None,
        }
    }

    pub(super) fn with_microsoft365_setup_issue(mut self, issue: SetupIssue) -> Self {
        self.microsoft365_setup_issue = Some(issue);
        self
    }
}

#[derive(Clone)]
enum AccountStatus {
    Restoring,
    Disconnected,
    Connecting {
        saved_credential_may_exist: bool,
        cancellation: AccountOperationCancellation,
    },
    CancellingConnect {
        saved_credential_may_exist: bool,
    },
    SigningOut {
        account: Option<ConnectedAccount>,
    },
    Connected(ConnectedAccount),
    Reauthorizing {
        account: ConnectedAccount,
        cancellation: AccountOperationCancellation,
    },
    CancellingReauthorization {
        account: ConnectedAccount,
    },
    ReauthorizationFailed {
        account: ConnectedAccount,
        error: AccountOperationError,
    },
    SignOutFailed {
        account: Option<ConnectedAccount>,
        error: AccountOperationError,
    },
    Failed {
        error: AccountOperationError,
        saved_credential_may_exist: bool,
    },
}

impl AccountStatus {
    fn saved_credential_may_exist(&self) -> bool {
        match self {
            Self::Connecting {
                saved_credential_may_exist,
                ..
            }
            | Self::CancellingConnect {
                saved_credential_may_exist,
            }
            | Self::Failed {
                saved_credential_may_exist,
                ..
            } => *saved_credential_may_exist,
            Self::SigningOut { .. }
            | Self::Connected(_)
            | Self::Reauthorizing { .. }
            | Self::CancellingReauthorization { .. }
            | Self::ReauthorizationFailed { .. }
            | Self::SignOutFailed { .. }
            | Self::Restoring => true,
            Self::Disconnected => false,
        }
    }

    fn connected_profile(&self) -> Option<&AccountProfile> {
        self.connected_account().map(ConnectedAccount::profile)
    }

    fn connected_account(&self) -> Option<&ConnectedAccount> {
        match self {
            Self::Connected(account)
            | Self::Reauthorizing { account, .. }
            | Self::CancellingReauthorization { account }
            | Self::ReauthorizationFailed { account, .. } => Some(account),
            Self::SigningOut {
                account: Some(account),
            }
            | Self::SignOutFailed {
                account: Some(account),
                ..
            } => Some(account),
            _ => None,
        }
    }

    fn error_message(error: AccountOperationError) -> &'static str {
        match error {
            AccountOperationError::SignInCancelled => "Sign-in was cancelled.",
            AccountOperationError::AccessDenied => {
                "The server denied access. Check the account details and your organization's policies."
            }
            AccountOperationError::InvalidConnectionDetails => {
                "The Exchange endpoint or sign-in details are invalid. Check them and try again."
            }
            AccountOperationError::AutodiscoverFailed => {
                "Autodiscover could not find an EWS endpoint. Enter the HTTPS address manually."
            }
            AccountOperationError::AccessTokenRejected => {
                "The server rejected the saved sign-in. Connect your account again."
            }
            AccountOperationError::SignInExpired => {
                "The saved sign-in has expired. Connect your account again."
            }
            AccountOperationError::CredentialStoreUnavailable => {
                "Linux Secret Service is unavailable. Unlock your keyring and try again."
            }
            AccountOperationError::ProfileUnavailable => {
                "The server did not return the account details needed to identify this account."
            }
            AccountOperationError::ServiceUnavailable => {
                "The account service is unavailable. Check your connection and try again."
            }
            AccountOperationError::SignInFailed => "Sign-in could not be completed. Try again.",
            AccountOperationError::AccountAlreadyConnected => "An account is already connected.",
            AccountOperationError::AccountNotConnected => "No account is connected.",
            AccountOperationError::AccountMismatch => {
                "Microsoft returned a different account. Restore the expected account before replacing this saved sign-in."
            }
            AccountOperationError::SavedAccountIdentityUnavailable => {
                "The saved sign-in has no verified account identity. Retry restore before removing it or signing in with another account."
            }
            AccountOperationError::AccountOperationInProgress => {
                "An account operation is already running. Wait for it to finish and try again."
            }
            AccountOperationError::AccountStateUnavailable => {
                "The account workflow is unavailable. Restart Einfach Mail and try again."
            }
            _ => "The account operation failed. Try again.",
        }
    }

    fn reauthorization_error_message(error: AccountOperationError) -> &'static str {
        match error {
            AccountOperationError::SignInCancelled => "Permission review was cancelled.",
            AccountOperationError::AccountMismatch => {
                "Microsoft returned a different account. Your saved sign-in was not changed."
            }
            _ => Self::error_message(error),
        }
    }
}

pub(super) struct AccountView<P: AccountProvider + 'static> {
    services: Result<AccountServices<P>, SetupIssue>,
    account_session: Option<Arc<dyn AccountSession>>,
    status: AccountStatus,
    pending_permission_review: Option<AccountPermissionRequest>,
    content: AccountContent,
    mail_navigation_focus: FocusHandle,
    calendar_navigation_focus: FocusHandle,
    accounts_navigation_focus: FocusHandle,
    session_open_error: Option<AccountViewCacheError>,
    exchange_ews_form: Option<ExchangeEwsConnectionForm>,
    task: Option<Task<()>>,
}

struct ExchangeEwsConnectionForm {
    endpoint: Entity<InputState>,
    mailbox_address: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    discovery: EwsDiscoveryState,
}

enum EwsDiscoveryState {
    Idle,
    Searching,
    Confirmation(ExchangeEwsDiscoveryConfirmation),
    Cancelling,
    Found,
    Failed,
}

impl EwsDiscoveryState {
    fn allows_start(&self) -> bool {
        matches!(self, Self::Idle | Self::Failed | Self::Found)
    }

    fn is_busy(&self) -> bool {
        matches!(self, Self::Searching | Self::Cancelling)
    }

    fn requires_confirmation(&self) -> bool {
        matches!(self, Self::Confirmation(_))
    }

    fn feedback_message(&self) -> Option<&'static str> {
        match self {
            Self::Idle | Self::Confirmation(_) => None,
            Self::Searching => Some("Searching for an EWS endpoint…"),
            Self::Cancelling => Some("Cancelling discovery…"),
            Self::Found => Some("Endpoint found. Review it before connecting."),
            Self::Failed => Some(
                "Autodiscover could not find an endpoint. Enter the HTTPS EWS address manually.",
            ),
        }
    }

    fn button_label(&self) -> &'static str {
        if self.is_busy() {
            "Discovering…"
        } else {
            "Discover endpoint"
        }
    }

    fn from_result(
        discovery_result: Result<ExchangeEwsDiscoveryResult, AccountOperationError>,
    ) -> Self {
        match discovery_result {
            Ok(ExchangeEwsDiscoveryResult::ConfirmationRequired(confirmation)) => {
                Self::Confirmation(confirmation)
            }
            Ok(ExchangeEwsDiscoveryResult::Cancelled) => Self::Idle,
            _ => Self::Failed,
        }
    }
}

impl ExchangeEwsConnectionForm {
    fn new<P: AccountProvider + 'static>(
        window: &mut Window,
        cx: &mut Context<AccountView<P>>,
    ) -> Self {
        Self {
            endpoint: cx.new(|cx| {
                InputState::new(window, cx).placeholder("https://mail.example/EWS/Exchange.asmx")
            }),
            mailbox_address: cx
                .new(|cx| InputState::new(window, cx).placeholder("name@example.com")),
            username: cx.new(|cx| InputState::new(window, cx).placeholder("DOMAIN\\username")),
            password: cx.new(|cx| InputState::new(window, cx).masked(true)),
            discovery: EwsDiscoveryState::Idle,
        }
    }

    fn request<P: AccountProvider + 'static>(
        &self,
        cx: &Context<AccountView<P>>,
    ) -> AccountConnectionRequest {
        AccountConnectionRequest::ExchangeEws(ExchangeEwsConnectionInput::new(
            self.endpoint.read(cx).value().to_string(),
            self.mailbox_address.read(cx).value().to_string(),
            self.username.read(cx).value().to_string(),
            self.password.read(cx).value().to_string(),
        ))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum OpeningContent {
    Mail,
    Calendar,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum WorkspaceSection {
    Mail,
    Calendar,
    Accounts,
}

impl WorkspaceSection {
    fn navigation_identity(self) -> (&'static str, &'static str, IconName) {
        match self {
            Self::Mail => ("nav-mail", "Mail", IconName::Inbox),
            Self::Calendar => ("nav-calendar", "Calendar", IconName::Calendar),
            Self::Accounts => ("nav-accounts", "Accounts", IconName::User),
        }
    }
}

enum AccountContent {
    Account,
    Opening(OpeningContent),
    Mail {
        view: Entity<MailView>,
        folder_navigation: Entity<MailFolderNavigation>,
        subscription: Subscription,
    },
    Calendar {
        view: Entity<CalendarView>,
        subscription: Subscription,
    },
}

impl AccountContent {
    fn is_account(&self) -> bool {
        matches!(self, Self::Account)
    }

    fn opening_kind(&self) -> Option<OpeningContent> {
        match self {
            Self::Opening(opening) => Some(*opening),
            Self::Account | Self::Mail { .. } | Self::Calendar { .. } => None,
        }
    }

    fn selected_section(&self) -> WorkspaceSection {
        match self {
            Self::Mail { .. } => WorkspaceSection::Mail,
            Self::Calendar { .. } => WorkspaceSection::Calendar,
            Self::Opening(OpeningContent::Mail) => WorkspaceSection::Mail,
            Self::Opening(OpeningContent::Calendar) => WorkspaceSection::Calendar,
            Self::Account => WorkspaceSection::Accounts,
        }
    }
}

enum AccountSignOutError {
    Identity(AccountOperationError),
    CacheRemoval(AccountCacheRemovalError),
    CredentialRemoval(AccountOperationError),
}

#[derive(Clone, Copy)]
enum AccountViewCacheError {
    Open(AccountCacheOpenError),
    Removal(AccountCacheRemovalError),
}

fn permission_review_disclosure(request: AccountPermissionRequest) -> &'static str {
    match request {
        AccountPermissionRequest::Current => {
            "Microsoft will review the existing delegated access: User.Read, Mail.Read, Calendars.Read, and offline_access."
        }
        AccountPermissionRequest::MailReadWrite => {
            "Microsoft grants Mail.ReadWrite, which can create, read, update, and delete mail in this account. Einfach Mail requests it when Microsoft denies a read-state change, follow-up flag change, message move, or message deletion. It uses the permission to set read state, update or clear follow-up flags, move messages between folders, and delete selected messages after confirmation. This permission review does not perform the action or send mail, and it does not request Mail.Send. Continue opens Microsoft in your system browser."
        }
        AccountPermissionRequest::CalendarReadWrite => {
            "Microsoft grants Calendars.ReadWrite the ability to create, read, update, and delete calendar events. Einfach Mail requests it for supported one-off timed appointments after Microsoft denies an event change. This review does not repeat the failed action. Continue opens Microsoft in your system browser. Reopen Calendar and retry the event after review."
        }
        AccountPermissionRequest::SendMail => {
            "Microsoft grants Mail.Send so Einfach Mail can send mail as this account. It does not grant permission to read or change mail. Continue opens Microsoft in your system browser."
        }
    }
}

impl<P: AccountProvider + 'static> AccountView<P> {
    pub(super) fn new(
        services: Result<AccountServices<P>, SetupIssue>,
        cx: &mut Context<Self>,
    ) -> Self {
        let status = if services.is_ok() {
            AccountStatus::Restoring
        } else {
            AccountStatus::Disconnected
        };
        let mut view = Self {
            services,
            account_session: None,
            status,
            pending_permission_review: None,
            content: AccountContent::Account,
            mail_navigation_focus: cx.focus_handle().tab_stop(true),
            calendar_navigation_focus: cx.focus_handle().tab_stop(true),
            accounts_navigation_focus: cx.focus_handle().tab_stop(true),
            session_open_error: None,
            exchange_ews_form: None,
            task: None,
        };
        view.start_restore(cx);
        view
    }

    fn start_restore(&mut self, cx: &mut Context<Self>) {
        let Ok(services) = &self.services else {
            return;
        };
        let workflow = Arc::clone(&services.workflow);
        self.account_session = None;
        self.status = AccountStatus::Restoring;
        self.session_open_error = None;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let restore_result = workflow.restore_account().await;
            let _ = this.update(cx, |view, cx| {
                view.task = None;
                view.status = match restore_result {
                    Ok(Some(account)) => AccountStatus::Connected(account),
                    Ok(None) => AccountStatus::Disconnected,
                    Err(error) => AccountStatus::Failed {
                        error,
                        saved_credential_may_exist: true,
                    },
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.begin_connect(AccountConnectionRequest::Microsoft365, cx);
    }

    fn begin_exchange_ews_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.task.is_some()
            || !matches!(
                &self.status,
                AccountStatus::Disconnected
                    | AccountStatus::Failed {
                        saved_credential_may_exist: false,
                        ..
                    }
            )
        {
            return;
        }
        self.exchange_ews_form = Some(ExchangeEwsConnectionForm::new(window, cx));
        cx.notify();
    }

    fn cancel_exchange_ews_form(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, AccountStatus::Connecting { .. }) {
            self.cancel_connect(cx);
        } else if !matches!(self.status, AccountStatus::CancellingConnect { .. }) {
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

    fn connect_exchange_ews(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.exchange_ews_form else {
            return;
        };
        let request = form.request(cx);
        self.begin_connect(request, cx);
    }

    fn discover_exchange_ews_endpoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn continue_exchange_ews_discovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirmation) = self.pending_ews_discovery_confirmation().cloned() else {
            return;
        };
        self.start_ews_discovery(
            ExchangeEwsDiscoveryRequest::Continue(confirmation),
            window,
            cx,
        );
    }

    fn cancel_exchange_ews_discovery(&mut self, cx: &mut Context<Self>) {
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
                    saved_credential_may_exist: false,
                    ..
                }
        ) {
            return;
        }
        let saved_credential_may_exist = self.status.saved_credential_may_exist();
        let cancellation = AccountOperationCancellation::new();
        self.status = AccountStatus::Connecting {
            saved_credential_may_exist,
            cancellation: cancellation.clone(),
        };
        self.task = Some(Self::connect_task(
            workflow,
            request,
            saved_credential_may_exist,
            cancellation,
            cx,
        ));
        cx.notify();
    }

    fn connect_task(
        workflow: Arc<AccountWorkflow<P>>,
        request: AccountConnectionRequest,
        saved_credential_may_exist: bool,
        cancellation: AccountOperationCancellation,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let connection_result = workflow
                .connect_account_with_request_and_cancellation(request, cancellation)
                .await;
            let _ = this.update(cx, |view, cx| {
                view.task = None;
                let saved_credential_may_exist = saved_credential_may_exist
                    || matches!(
                        &connection_result,
                        Err(AccountOperationError::SavedAccountIdentityUnavailable
                            | AccountOperationError::AccountMismatch)
                    );
                view.status = match connection_result {
                    Ok(account) => {
                        view.exchange_ews_form = None;
                        AccountStatus::Connected(account)
                    }
                    Err(error) => AccountStatus::Failed {
                        error,
                        saved_credential_may_exist,
                    },
                };
                cx.notify();
            });
        })
    }

    fn cancel_connect(&mut self, cx: &mut Context<Self>) {
        let AccountStatus::Connecting {
            saved_credential_may_exist,
            cancellation,
        } = &self.status
        else {
            return;
        };
        cancellation.cancel();
        self.status = AccountStatus::CancellingConnect {
            saved_credential_may_exist: *saved_credential_may_exist,
        };
        cx.notify();
    }

    fn prepare_permission_review(
        &mut self,
        request: AccountPermissionRequest,
        cx: &mut Context<Self>,
    ) {
        if self.content.opening_kind().is_some() || self.status.connected_profile().is_none() {
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

    fn continue_permission_review(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.pending_permission_review.take() else {
            return;
        };
        self.reauthorize(request, cx);
    }

    fn cancel_permission_review(&mut self, cx: &mut Context<Self>) {
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

    fn cancel_reauthorization(&mut self, cx: &mut Context<Self>) {
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

    fn sign_out(&mut self, cx: &mut Context<Self>) {
        let Ok(services) = &self.services else {
            return;
        };
        let workflow = Arc::clone(&services.workflow);
        if !self.status.saved_credential_may_exist()
            || self.content.opening_kind().is_some()
            || matches!(
                &self.status,
                AccountStatus::Connecting { .. }
                    | AccountStatus::CancellingConnect { .. }
                    | AccountStatus::Reauthorizing { .. }
                    | AccountStatus::CancellingReauthorization { .. }
                    | AccountStatus::SigningOut { .. }
            )
        {
            return;
        }

        let previous_status = self.status.clone();
        let account = previous_status.connected_account().cloned();
        let account_session_factory = Arc::clone(&services.account_session_factory);
        let account_session = self
            .close_content_view(cx)
            .or_else(|| self.account_session.clone());
        self.content = AccountContent::Account;
        self.session_open_error = None;
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

    fn close_content_view(&mut self, cx: &mut Context<Self>) -> Option<Arc<dyn AccountSession>> {
        match std::mem::replace(&mut self.content, AccountContent::Account) {
            AccountContent::Mail {
                view,
                folder_navigation,
                subscription,
            } => {
                drop(subscription);
                drop(folder_navigation);
                view.update(cx, |view, _| view.stop_pending_work());
            }
            AccountContent::Calendar { view, subscription } => {
                drop(subscription);
                view.update(cx, |view, _| view.stop_pending_work());
            }
            content => {
                self.content = content;
                return None;
            }
        }
        self.account_session.clone()
    }

    fn open_mail(&mut self, cx: &mut Context<Self>) {
        let Ok(services) = &self.services else {
            return;
        };
        let permission_requester: Arc<dyn MailPermissionRequester> = services.workflow.clone();
        self.open_account_content(
            OpeningContent::Mail,
            move |profile, session, cx| {
                let mail_view_session = session.mail_session();
                let mail_view = cx
                    .new(|cx| MailView::new(profile, mail_view_session, permission_requester, cx));
                let folder_navigation =
                    cx.new(|cx| MailFolderNavigation::new(mail_view.clone(), cx));
                let subscription = cx.subscribe(
                    &mail_view,
                    |view, _, event: &MailViewEvent, cx| match event {
                        MailViewEvent::ReviewMailWritePermission => {
                            drop(view.close_content_view(cx));
                            view.prepare_permission_review(
                                AccountPermissionRequest::MailReadWrite,
                                cx,
                            );
                        }
                    },
                );
                AccountContent::Mail {
                    view: mail_view,
                    folder_navigation,
                    subscription,
                }
            },
            cx,
        );
    }

    fn open_calendar(&mut self, cx: &mut Context<Self>) {
        if !self.calendar_is_available() {
            return;
        }
        self.open_account_content(
            OpeningContent::Calendar,
            |profile, session, cx| {
                let calendar_session = session.calendar_session();
                let calendar_view = cx.new(|cx| CalendarView::new(profile, calendar_session, cx));
                let subscription =
                    cx.subscribe(&calendar_view, |view, _, event: &CalendarViewEvent, cx| {
                        match event {
                            CalendarViewEvent::ReviewCalendarWritePermission => {
                                drop(view.close_content_view(cx));
                                view.prepare_permission_review(
                                    AccountPermissionRequest::CalendarReadWrite,
                                    cx,
                                );
                            }
                        }
                    });
                AccountContent::Calendar {
                    view: calendar_view,
                    subscription,
                }
            },
            cx,
        );
    }

    fn navigate_to(&mut self, section: WorkspaceSection, cx: &mut Context<Self>) {
        if self.content.selected_section() == section
            || self.navigation_disabled_reason(section, cx).is_some()
        {
            return;
        }

        drop(self.close_content_view(cx));
        match section {
            WorkspaceSection::Accounts => cx.notify(),
            WorkspaceSection::Mail => self.open_mail(cx),
            WorkspaceSection::Calendar => self.open_calendar(cx),
        }
    }

    fn navigation_disabled_reason(
        &self,
        section: WorkspaceSection,
        cx: &App,
    ) -> Option<&'static str> {
        if self.content.opening_kind().is_some() {
            return Some("Wait for the current section to finish opening.");
        }
        if !self.can_navigate_away(cx) {
            return Some("Wait for the current mail update to finish before switching sections.");
        }
        self.section_disabled_reason(section)
    }

    fn section_disabled_reason(&self, section: WorkspaceSection) -> Option<&'static str> {
        if section == WorkspaceSection::Accounts {
            return None;
        }
        if let Err(issue) = &self.services {
            return Some(issue.message());
        }
        if !matches!(&self.status, AccountStatus::Connected(_)) {
            return Some(if matches!(&self.status, AccountStatus::Disconnected) {
                "Connect an account to use this section."
            } else {
                "Resolve the account status in Accounts before using this section."
            });
        }
        if section == WorkspaceSection::Calendar && !self.calendar_is_available() {
            return Some("Calendar is unavailable for this account.");
        }
        None
    }

    fn calendar_is_available(&self) -> bool {
        self.status
            .connected_profile()
            .is_some_and(|profile| profile.capabilities().calendar() != CalendarAccess::Unavailable)
    }

    fn navigation_focus(&self, section: WorkspaceSection) -> &FocusHandle {
        match section {
            WorkspaceSection::Mail => &self.mail_navigation_focus,
            WorkspaceSection::Calendar => &self.calendar_navigation_focus,
            WorkspaceSection::Accounts => &self.accounts_navigation_focus,
        }
    }

    fn can_navigate_away(&self, cx: &App) -> bool {
        match &self.content {
            AccountContent::Mail { view, .. } => view.read(cx).can_navigate_away(),
            AccountContent::Account
            | AccountContent::Opening(_)
            | AccountContent::Calendar { .. } => true,
        }
    }

    fn open_account_content(
        &mut self,
        opening: OpeningContent,
        build_content: impl FnOnce(
            AccountProfile,
            Arc<dyn AccountSession>,
            &mut Context<Self>,
        ) -> AccountContent
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if !self.content.is_account() {
            return;
        }
        let AccountStatus::Connected(account) = &self.status else {
            return;
        };
        let Ok(services) = &self.services else {
            return;
        };
        let account_session_factory = Arc::clone(&services.account_session_factory);
        let profile = account.profile().clone();
        let retained_session = self.account_session.clone();
        self.content = AccountContent::Opening(opening);
        self.session_open_error = None;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let session = match retained_session {
                Some(session) => Ok(session),
                None => {
                    let account_id = profile.id().clone();
                    cx.background_executor()
                        .spawn(
                            async move { account_session_factory.open_session(account_id).await },
                        )
                        .await
                }
            };
            let _ = this.update(cx, move |view, cx| {
                if view.content.opening_kind() != Some(opening)
                    || !matches!(
                        &view.status,
                        AccountStatus::Connected(current_profile)
                            if current_profile.profile().id() == profile.id()
                    )
                {
                    return;
                }
                view.task = None;
                view.content = AccountContent::Account;
                match session {
                    Ok(session) => {
                        view.content = build_content(profile, Arc::clone(&session), cx);
                        view.account_session = Some(session);
                    }
                    Err(error) => {
                        view.session_open_error = Some(AccountViewCacheError::Open(error));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
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
        match result {
            Ok(()) => {
                self.account_session = None;
                self.status = AccountStatus::Disconnected;
            }
            Err(AccountSignOutError::Identity(error)) => {
                self.status = AccountStatus::Failed {
                    error,
                    saved_credential_may_exist: true,
                };
            }
            Err(AccountSignOutError::CacheRemoval(error)) => {
                self.status = previous_status;
                self.session_open_error = Some(AccountViewCacheError::Removal(error));
                if !matches!(
                    error,
                    AccountCacheRemovalError::OperationInProgress
                        | AccountCacheRemovalError::PendingMailSend
                        | AccountCacheRemovalError::PendingCalendarEventCreation
                ) {
                    self.account_session = None;
                }
            }
            Err(AccountSignOutError::CredentialRemoval(error)) => {
                self.account_session = None;
                self.status = AccountStatus::SignOutFailed {
                    account: previous_status.connected_account().cloned(),
                    error,
                };
            }
        }
        self.task = None;
        cx.notify();
    }

    fn account_identity(&self, profile: &AccountProfile, colors: ColorTokens) -> impl IntoElement {
        let mut identity = div()
            .id("connected-account")
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .rounded_lg()
            .child(
                div()
                    .text_base()
                    .font_bold()
                    .child(profile.label().to_owned()),
            );

        if let Some(mailbox) = profile.mailbox() {
            identity = identity.child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(format!("Mailbox: {}", mailbox.address())),
            );
        }
        if let Some(user_principal_name) = profile
            .user_principal_name()
            .filter(|upn| !profile.label().eq_ignore_ascii_case(upn))
        {
            identity = identity.child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(format!("Sign-in: {user_principal_name}")),
            );
        }

        identity
    }

    fn render_permissions(&self, colors: ColorTokens) -> impl IntoElement {
        div()
            .id("account-requested-access")
            .debug_selector(|| "account-requested-access".to_owned())
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .rounded_lg()
            .child(div().text_base().font_bold().child("Requested access"))
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child("Initial sign-in requests User.Read, Mail.Read, Calendars.Read, and offline_access. Mail.Read lets Einfach Mail read your mail, and Calendars.Read lets it read your primary calendar. If Microsoft denies a read-state change, follow-up flag change, message move, or message deletion, a separate review can request Mail.ReadWrite, which permits creating, reading, updating, and deleting mail. The app uses this access to set messages read or unread, update or clear follow-up flags, move messages between folders, and delete selected messages after confirmation. If Microsoft denies a supported calendar change, a separate review can request Calendars.ReadWrite, which permits creating, reading, updating, and deleting calendar events. Einfach Mail uses it to create one-off timed appointments, update the title or time of supported standalone appointments you organize that have no attendees, and delete those appointments after confirmation. It does not create recurring or all-day events, send invitations, or create online meetings. The app does not request Mail.Send."),
            )
    }

    fn render_status(&self) -> impl IntoElement {
        let (label, message, role) = match &self.status {
            AccountStatus::Restoring => (
                "Checking for a saved account",
                "Checking for a saved account…",
                Role::Status,
            ),
            AccountStatus::Disconnected => {
                ("Not connected", "No account is connected.", Role::Status)
            }
            AccountStatus::Connecting { .. } => (
                "Connecting to account",
                "Connecting to account…",
                Role::Status,
            ),
            AccountStatus::CancellingConnect { .. } => (
                "Finishing account connection",
                "Finishing connection…",
                Role::Status,
            ),
            AccountStatus::SigningOut { .. } => (
                "Removing saved sign-in",
                "Removing saved sign-in…",
                Role::Status,
            ),
            AccountStatus::SignOutFailed { error, .. } => (
                AccountStatus::error_message(*error),
                AccountStatus::error_message(*error),
                Role::Alert,
            ),
            AccountStatus::Reauthorizing { .. } => (
                "Reviewing Microsoft permissions",
                "Waiting for Microsoft permission review in your browser…",
                Role::Status,
            ),
            AccountStatus::CancellingReauthorization { .. } => (
                "Finishing Microsoft permission review",
                "Finishing permission review…",
                Role::Status,
            ),
            AccountStatus::ReauthorizationFailed { error, .. } => (
                AccountStatus::reauthorization_error_message(*error),
                AccountStatus::reauthorization_error_message(*error),
                Role::Alert,
            ),
            AccountStatus::Connected(_) => ("Connected", "Connected", Role::Status),
            AccountStatus::Failed { error, .. } => (
                AccountStatus::error_message(*error),
                AccountStatus::error_message(*error),
                Role::Alert,
            ),
        };
        let label = match &self.status {
            AccountStatus::Connected(account) => {
                format!("Connected to {}", account.profile().label())
            }
            _ => label.to_owned(),
        };

        div()
            .id("account-status")
            .role(role)
            .aria_label(label)
            .text_sm()
            .child(message)
    }

    fn render_account_details(&self, colors: ColorTokens) -> Option<impl IntoElement> {
        let account = self.status.connected_account()?;
        let profile = account.profile();
        let provider = match account.provider_kind() {
            AccountProviderKind::MicrosoftGraph => "Microsoft Graph (Microsoft 365)",
            AccountProviderKind::ExchangeEws => "Exchange Web Services (Exchange Server)",
            _ => "Other provider",
        };
        let mut details = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.account_identity(profile, colors))
            .child(
                div()
                    .id("account-provider")
                    .debug_selector(|| "account-provider".to_owned())
                    .text_sm()
                    .child(format!("Provider: {provider}")),
            )
            .child(
                div()
                    .id("account-mail-capabilities")
                    .debug_selector(|| "account-mail-capabilities".to_owned())
                    .text_sm()
                    .child(mail_access_summary(profile.capabilities().mail())),
            );
        let calendar_summary = match profile.capabilities().calendar() {
            CalendarAccess::Unavailable => "Calendar actions: unavailable.",
            CalendarAccess::ReadOnly => "Calendar actions: view events.",
            CalendarAccess::ReadWrite => {
                "Calendar actions: view events and make supported changes."
            }
        };
        details = details.child(
            div()
                .id("account-calendar-capabilities")
                .debug_selector(|| "account-calendar-capabilities".to_owned())
                .text_sm()
                .child(calendar_summary),
        );
        Some(details.child(
            div()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("Signing out removes the saved sign-in and encrypted account cache from this device."),
        ))
    }

    fn render_saved_sign_out(
        &self,
        cx: &mut Context<Self>,
        label: &'static str,
    ) -> impl IntoElement {
        div()
            .flex()
            .justify_center()
            .debug_selector(|| "sign-out".to_owned())
            .child(
                Button::new("sign-out")
                    .danger()
                    .label(label)
                    .disabled(self.content.opening_kind().is_some())
                    .on_click(cx.listener(|view, _, _, cx| view.sign_out(cx))),
            )
    }

    fn render_permission_review(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(request) = self.pending_permission_review {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .id("permission-review-disclosure")
                .debug_selector(|| "permission-review-disclosure".to_owned())
                .role(Role::Alert)
                .child(permission_review_disclosure(request))
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .gap_2()
                        .child(
                            Button::new("continue-permission-review")
                                .debug_selector(|| "continue-permission-review".to_owned())
                                .primary()
                                .label("Continue to Microsoft")
                                .disabled(
                                    self.task.is_some() || self.content.opening_kind().is_some(),
                                )
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.continue_permission_review(cx);
                                })),
                        )
                        .child(
                            Button::new("cancel-permission-review")
                                .debug_selector(|| "cancel-permission-review".to_owned())
                                .ghost()
                                .label("Cancel")
                                .disabled(self.task.is_some())
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.cancel_permission_review(cx);
                                })),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .justify_center()
                .child(
                    Button::new("review-app-permissions")
                        .debug_selector(|| "review-app-permissions".to_owned())
                        .label("Review sign-in permissions")
                        .disabled(self.content.opening_kind().is_some() || self.task.is_some())
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.prepare_permission_review(AccountPermissionRequest::Current, cx);
                        })),
                )
                .into_any_element()
        }
    }

    fn render_progress_action(label: &'static str) -> gpui_kit::Div {
        div().flex().justify_center().child(
            Button::new("account-operation-progress")
                .debug_selector(|| "account-operation-progress".to_owned())
                .label(label)
                .loading(true)
                .disabled(true),
        )
    }

    fn render_primary_action(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match &self.status {
            AccountStatus::Restoring => Self::render_progress_action("Checking account…"),
            AccountStatus::SigningOut { .. } => Self::render_progress_action("Signing out…"),
            AccountStatus::SignOutFailed { .. } => div(),
            AccountStatus::CancellingConnect { .. } => {
                Self::render_progress_action("Finishing sign-in…")
            }
            AccountStatus::Reauthorizing { .. } => div()
                .flex()
                .justify_center()
                .debug_selector(|| "cancel-permission-update".to_owned())
                .child(
                    Button::new("cancel-permission-update")
                        .ghost()
                        .label("Cancel permission review")
                        .on_click(cx.listener(|view, _, _, cx| view.cancel_reauthorization(cx))),
                ),
            AccountStatus::CancellingReauthorization { .. } => {
                Self::render_progress_action("Finishing permission review…")
            }
            AccountStatus::Connecting { .. } => div()
                .flex()
                .justify_center()
                .debug_selector(|| "cancel-sign-in".to_owned())
                .child(
                    Button::new("cancel-sign-in")
                        .ghost()
                        .label("Cancel sign-in")
                        .on_click(cx.listener(|view, _, _, cx| view.cancel_connect(cx))),
                ),
            AccountStatus::Connected(_) if self.services.is_ok() => {
                match self.content.opening_kind() {
                    Some(OpeningContent::Mail) => Self::render_progress_action("Opening mail…"),
                    Some(OpeningContent::Calendar) => {
                        Self::render_progress_action("Opening calendar…")
                    }
                    None => div(),
                }
            }
            AccountStatus::Failed {
                saved_credential_may_exist: true,
                ..
            } => div()
                .flex()
                .justify_center()
                .debug_selector(|| "retry-account-restore".to_owned())
                .child(
                    Button::new("retry-account-restore")
                        .primary()
                        .label("Retry account restore")
                        .on_click(cx.listener(|view, _, _, cx| view.start_restore(cx))),
                ),
            AccountStatus::Disconnected
            | AccountStatus::Failed {
                saved_credential_may_exist: false,
                ..
            } => self.render_connect_options(colors, cx),
            AccountStatus::Connected(_) | AccountStatus::ReauthorizationFailed { .. } => div(),
        }
    }

    fn render_connect_options(&self, colors: ColorTokens, cx: &mut Context<Self>) -> gpui_kit::Div {
        div()
            .w_full()
            .max_w_48()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.render_microsoft365_choice(colors, cx))
            .child(self.render_exchange_ews_choice(colors, cx))
    }

    fn render_microsoft365_choice(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let mut choice = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_base().font_semibold().child("Microsoft 365"))
            .child(Self::render_microsoft365_provider_description(colors));
        if matches!(
            &self.services,
            Ok(services) if services.microsoft365_setup_issue.is_none()
        ) {
            choice = choice.child(self.render_permissions(colors));
        }
        self.add_microsoft365_connect_option(choice, cx)
    }

    fn render_microsoft365_provider_description(colors: ColorTokens) -> Stateful<Div> {
        div()
            .id("microsoft365-provider-description")
            .debug_selector(|| "microsoft365-provider-description".to_owned())
            .text_sm()
            .text_color(colors.muted_foreground)
            .child(
                "Connect a work or school Microsoft 365 account with Microsoft Graph through the system browser.",
            )
    }

    fn render_exchange_ews_choice(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_base().font_semibold().child("Exchange Server"))
            .child(Self::render_exchange_ews_provider_description(colors))
            .child(self.render_ews_connect_button(cx))
    }

    fn render_exchange_ews_provider_description(colors: ColorTokens) -> Stateful<Div> {
        div()
            .id("exchange-ews-provider-description")
            .debug_selector(|| "exchange-ews-provider-description".to_owned())
            .text_sm()
            .text_color(colors.muted_foreground)
            .child(
                "Connect to an on-premises Exchange Server with EWS. Enter an HTTPS endpoint or discover one from your mailbox.",
            )
    }

    fn add_microsoft365_connect_option(
        &self,
        options: gpui_kit::Div,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let Ok(services) = &self.services else {
            return options;
        };
        self.add_microsoft365_setup_action(options, services.microsoft365_setup_issue, cx)
    }

    fn add_microsoft365_setup_action(
        &self,
        options: gpui_kit::Div,
        issue: Option<SetupIssue>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        match issue {
            Some(issue) => options.child(Self::render_microsoft365_setup_status(issue)),
            None => options.child(self.render_microsoft365_connect_button(cx)),
        }
    }

    fn render_microsoft365_setup_status(issue: SetupIssue) -> Stateful<Div> {
        div()
            .id("microsoft365-setup-status")
            .role(Role::Status)
            .text_sm()
            .child(issue.message())
    }

    fn render_microsoft365_connect_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("connect-account")
            .debug_selector(|| "connect-account".to_owned())
            .label("Connect Microsoft 365 account")
            .on_click(cx.listener(|view, _, _, cx| view.connect(cx)))
    }

    fn render_ews_connect_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("connect-exchange-ews")
            .debug_selector(|| "connect-exchange-ews".to_owned())
            .label("Connect Exchange Server…")
            .disabled(self.task.is_some())
            .on_click(cx.listener(|view, _, window, cx| {
                view.begin_exchange_ews_form(window, cx);
            }))
    }

    fn render_exchange_ews_form(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let Some(form) = &self.exchange_ews_form else {
            return div();
        };
        div()
            .w_full()
            .max_w_48()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_lg().font_bold().child("Exchange Server"))
            .child(
                div().text_sm().child(
                    "Enter your server's HTTPS EWS address or discover it from your mailbox. Discovery sends the mailbox address without credentials. Review the endpoint before connecting; your NTLM username and password are sent only after you press Connect.",
                ),
            )
            .child(self.render_ews_form(form, cx))
    }

    fn render_ews_form(&self, form: &ExchangeEwsConnectionForm, cx: &mut Context<Self>) -> Form {
        Form::new()
            .child(self.render_ews_endpoint_field(form, cx))
            .children(self.render_ews_identity_fields(form))
            .footer(self.render_ews_form_footer(form, cx))
    }

    fn render_ews_endpoint_field(
        &self,
        form: &ExchangeEwsConnectionForm,
        cx: &mut Context<Self>,
    ) -> Field {
        Field::new().label("EWS endpoint").required(true).child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(self.render_ews_endpoint_input(form))
                .child(self.render_ews_discovery_button(form, cx))
                .child(self.render_ews_discovery_feedback(&form.discovery, cx)),
        )
    }

    fn render_ews_endpoint_input(&self, form: &ExchangeEwsConnectionForm) -> Stateful<Div> {
        div()
            .id("ews-endpoint-input")
            .debug_selector(|| "ews-endpoint-input".to_owned())
            .child(
                Input::new(&form.endpoint)
                    .aria_label("EWS endpoint")
                    .disabled(self.ews_inputs_disabled(form))
                    .w_full(),
            )
    }

    fn render_ews_discovery_button(
        &self,
        form: &ExchangeEwsConnectionForm,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Button::new("discover-ews-endpoint")
            .debug_selector(|| "discover-ews-endpoint".to_owned())
            .outline()
            .label(form.discovery.button_label())
            .loading(form.discovery.is_busy())
            .disabled(self.ews_discovery_button_disabled(&form.discovery))
            .on_click(cx.listener(|view, _, window, cx| {
                view.discover_exchange_ews_endpoint(window, cx);
            }))
    }

    fn ews_discovery_button_disabled(&self, state: &EwsDiscoveryState) -> bool {
        self.task.is_some()
            || self.ews_connection_busy()
            || state.is_busy()
            || state.requires_confirmation()
    }

    fn render_ews_discovery_feedback(
        &self,
        state: &EwsDiscoveryState,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if let EwsDiscoveryState::Confirmation(confirmation) = state {
            return self.render_ews_discovery_confirmation(confirmation, cx);
        }
        state.feedback_message().map_or_else(
            || div().id("ews-discovery-status"),
            Self::render_ews_discovery_message,
        )
    }

    fn render_ews_discovery_message(message: &'static str) -> Stateful<Div> {
        div()
            .id("ews-discovery-status")
            .role(Role::Status)
            .text_sm()
            .child(message)
    }

    fn render_ews_discovery_confirmation(
        &self,
        confirmation: &ExchangeEwsDiscoveryConfirmation,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id("ews-discovery-status")
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::render_ews_confirmation_prompt(confirmation))
            .child(self.render_ews_confirmation_actions(cx))
    }

    fn render_ews_confirmation_prompt(
        confirmation: &ExchangeEwsDiscoveryConfirmation,
    ) -> Stateful<Div> {
        div()
            .id("ews-discovery-confirmation-status")
            .role(Role::Status)
            .text_sm()
            .child(format!(
                "Send {} to {} to continue Autodiscover?",
                confirmation.mailbox_address(),
                confirmation.destination()
            ))
    }

    fn render_ews_confirmation_actions(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        div()
            .flex()
            .gap_2()
            .child(self.render_continue_ews_discovery_button(cx))
            .child(self.render_cancel_ews_discovery_button(cx))
    }

    fn render_continue_ews_discovery_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("continue-ews-discovery")
            .debug_selector(|| "continue-ews-discovery".to_owned())
            .primary()
            .label("Continue")
            .on_click(cx.listener(|view, _, window, cx| {
                view.continue_exchange_ews_discovery(window, cx);
            }))
    }

    fn render_cancel_ews_discovery_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("cancel-ews-discovery")
            .debug_selector(|| "cancel-ews-discovery".to_owned())
            .outline()
            .label("Cancel")
            .on_click(cx.listener(|view, _, _, cx| {
                view.cancel_exchange_ews_discovery(cx);
            }))
    }

    fn render_ews_identity_fields(&self, form: &ExchangeEwsConnectionForm) -> [Field; 3] {
        let inputs_disabled = self.ews_inputs_disabled(form);
        [
            self.render_ews_mailbox_field(form, inputs_disabled),
            self.render_ews_username_field(form, inputs_disabled),
            self.render_ews_password_field(form, inputs_disabled),
        ]
    }

    fn render_ews_mailbox_field(&self, form: &ExchangeEwsConnectionForm, disabled: bool) -> Field {
        Field::new().label("Mailbox address").required(true).child(
            div()
                .id("ews-mailbox-address-input")
                .debug_selector(|| "ews-mailbox-address-input".to_owned())
                .child(
                    Input::new(&form.mailbox_address)
                        .aria_label("Mailbox address")
                        .disabled(disabled)
                        .w_full(),
                ),
        )
    }

    fn render_ews_username_field(&self, form: &ExchangeEwsConnectionForm, disabled: bool) -> Field {
        Field::new().label("NTLM username").required(true).child(
            div()
                .id("ews-username-input")
                .debug_selector(|| "ews-username-input".to_owned())
                .child(
                    Input::new(&form.username)
                        .aria_label("NTLM username")
                        .disabled(disabled)
                        .w_full(),
                ),
        )
    }

    fn render_ews_password_field(&self, form: &ExchangeEwsConnectionForm, disabled: bool) -> Field {
        Field::new().label("Password").required(true).child(
            div()
                .id("ews-password-input")
                .debug_selector(|| "ews-password-input".to_owned())
                .child(
                    Input::new(&form.password)
                        .aria_label("Password")
                        .mask_toggle()
                        .disabled(disabled)
                        .w_full(),
                ),
        )
    }

    fn render_ews_form_footer(
        &self,
        form: &ExchangeEwsConnectionForm,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        div()
            .flex()
            .gap_2()
            .child(self.render_cancel_ews_connection_button(cx))
            .child(self.render_submit_ews_connection_button(form, cx))
    }

    fn render_cancel_ews_connection_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let connecting = self.ews_connection_busy();
        let cancelling = matches!(&self.status, AccountStatus::CancellingConnect { .. });
        let label = if cancelling {
            "Finishing connection…"
        } else if connecting {
            "Cancel connection"
        } else {
            "Cancel"
        };
        Button::new("cancel-ews-connection-form")
            .outline()
            .label(label)
            .disabled(cancelling)
            .on_click(cx.listener(|view, _, _, cx| {
                view.cancel_exchange_ews_form(cx);
            }))
    }

    fn render_submit_ews_connection_button(
        &self,
        form: &ExchangeEwsConnectionForm,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let connecting = matches!(&self.status, AccountStatus::Connecting { .. });
        Button::new("submit-ews-connection")
            .debug_selector(|| "submit-ews-connection".to_owned())
            .primary()
            .label(self.ews_submit_label())
            .loading(connecting)
            .disabled(self.ews_inputs_disabled(form))
            .on_click(cx.listener(|view, _, _, cx| {
                view.connect_exchange_ews(cx);
            }))
    }

    fn ews_submit_label(&self) -> &'static str {
        if matches!(&self.status, AccountStatus::CancellingConnect { .. }) {
            "Finishing connection…"
        } else if matches!(&self.status, AccountStatus::Connecting { .. }) {
            "Connecting…"
        } else {
            "Connect"
        }
    }

    fn ews_inputs_disabled(&self, form: &ExchangeEwsConnectionForm) -> bool {
        self.ews_connection_busy()
            || form.discovery.is_busy()
            || form.discovery.requires_confirmation()
    }

    fn ews_connection_busy(&self) -> bool {
        matches!(
            &self.status,
            AccountStatus::Connecting { .. } | AccountStatus::CancellingConnect { .. }
        )
    }

    fn render_header(&self, colors: ColorTokens) -> gpui_kit::Div {
        let description = self.account_header_description();
        div()
            .w_full()
            .max_w_48()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_lg().font_bold().child("Accounts"))
            .child(
                div()
                    .id("account-header-description")
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(description),
            )
    }

    fn account_header_description(&self) -> &'static str {
        if self.exchange_ews_form.is_some() {
            return "Complete or cancel the Exchange Server connection below.";
        }

        Self::account_status_description(&self.status)
    }

    fn account_status_description(status: &AccountStatus) -> &'static str {
        Self::saved_sign_in_description(status)
            .or_else(|| Self::operation_description(status))
            .unwrap_or("Choose Microsoft 365 or Exchange Server to connect an account.")
    }

    fn saved_sign_in_description(status: &AccountStatus) -> Option<&'static str> {
        match status {
            AccountStatus::Failed {
                saved_credential_may_exist: true,
                error:
                    AccountOperationError::SavedAccountIdentityUnavailable
                    | AccountOperationError::AccountMismatch,
                ..
            } => Some("Retry account restore before removing or replacing the saved sign-in."),
            AccountStatus::Failed {
                saved_credential_may_exist: true,
                ..
            } => Some("Restore or remove the saved sign-in before connecting another account."),
            _ => None,
        }
    }

    fn operation_description(status: &AccountStatus) -> Option<&'static str> {
        Self::sign_in_operation_description(status)
            .or_else(|| Self::sign_out_operation_description(status))
            .or_else(|| Self::connected_account_description(status))
    }

    fn sign_in_operation_description(status: &AccountStatus) -> Option<&'static str> {
        match status {
            AccountStatus::Restoring => Some("Checking for a saved account."),
            AccountStatus::Connecting { .. } => {
                Some("Connection is in progress. Cancel it below if needed.")
            }
            AccountStatus::CancellingConnect { .. } => {
                Some("Connection cancellation is finishing. Wait before starting another sign-in.")
            }
            _ => None,
        }
    }

    fn sign_out_operation_description(status: &AccountStatus) -> Option<&'static str> {
        match status {
            AccountStatus::SigningOut { account: Some(_) } => {
                Some("Sign-out is in progress. Account details remain visible until it finishes.")
            }
            AccountStatus::SigningOut { account: None } => Some(
                "Saved sign-in removal is in progress. Wait before connecting another account.",
            ),
            AccountStatus::SignOutFailed { .. } => {
                Some("Sign-out needs attention. Retry it below.")
            }
            _ => None,
        }
    }

    fn connected_account_description(status: &AccountStatus) -> Option<&'static str> {
        match status {
            AccountStatus::Connected(_)
            | AccountStatus::Reauthorizing { .. }
            | AccountStatus::CancellingReauthorization { .. }
            | AccountStatus::ReauthorizationFailed { .. } => {
                Some("Manage the active account, review permissions, or sign out.")
            }
            _ => None,
        }
    }

    fn render_setup_status(&self, issue: SetupIssue) -> impl IntoElement {
        let message = issue.message();
        div()
            .id("account-status")
            .role(Role::Status)
            .aria_label(message)
            .text_sm()
            .child(message)
    }

    fn render_account_content(&self, colors: ColorTokens, cx: &mut Context<Self>) -> gpui_kit::Div {
        let content = self.render_header(colors);
        if let Err(issue) = &self.services {
            return content.child(self.render_setup_status(*issue));
        }
        let mut content = content.child(self.render_status());
        if let Some(error) = self.session_open_error {
            let message = match error {
                AccountViewCacheError::Open(error) => account_cache_open_error_message(error),
                AccountViewCacheError::Removal(error) => account_cache_removal_error_message(error),
            };
            content = content.child(
                div()
                    .id("account-cache-error")
                    .debug_selector(|| "account-cache-error".to_owned())
                    .role(Role::Alert)
                    .text_sm()
                    .child(message),
            );
        }
        self.render_account_actions(content, colors, cx)
    }

    fn render_account_actions(
        &self,
        mut content: gpui_kit::Div,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        if self.exchange_ews_form.is_some() {
            return content.child(self.render_exchange_ews_form(cx));
        }
        if let Some(details) = self.render_account_details(colors) {
            content = content.child(details);
        }
        if matches!(
            &self.status,
            AccountStatus::Connected(_) | AccountStatus::ReauthorizationFailed { .. }
        ) {
            if self
                .status
                .connected_profile()
                .is_some_and(|profile| profile.capabilities().can_review_mail_write_permission())
            {
                content = content.child(self.render_permission_review(cx));
            }
            if self.pending_permission_review.is_none() {
                content = content.child(self.render_saved_sign_out(cx, "Sign out"));
            }
        }
        if matches!(&self.status, AccountStatus::SignOutFailed { .. }) {
            return content.child(self.render_saved_sign_out(cx, "Retry sign out"));
        }
        if matches!(
            &self.status,
            AccountStatus::Failed {
                saved_credential_may_exist: true,
                error: AccountOperationError::SavedAccountIdentityUnavailable
                    | AccountOperationError::AccountMismatch,
                ..
            }
        ) {
            return content.child(self.render_primary_action(colors, cx));
        }
        if matches!(
            &self.status,
            AccountStatus::Failed {
                saved_credential_may_exist: true,
                ..
            }
        ) {
            content = content.child(self.render_saved_sign_out(cx, "Remove saved sign-in"));
        }
        content.child(self.render_primary_action(colors, cx))
    }

    fn render_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let navigation = div()
            .id("workspace-navigation")
            .role(Role::Navigation)
            .aria_label("Main navigation")
            .w(rems(16.))
            .min_w(rems(14.))
            .max_w(rems(20.))
            .h_full()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .bg(cx.theme().tokens.sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(self.render_navigation_brand())
            .child(self.render_feature_navigation(cx));
        #[cfg(test)]
        let navigation = navigation.test_support();
        let mut navigation = navigation;
        if let Some(profile) = self.status.connected_profile() {
            navigation = navigation.child(Self::render_active_account_identity(profile, cx));
        }
        if let AccountContent::Mail {
            folder_navigation, ..
        } = &self.content
        {
            navigation = navigation.child(folder_navigation.clone());
        } else {
            navigation = navigation.child(div().flex_1());
        }
        navigation.child(self.render_account_navigation(cx))
    }

    fn render_navigation_brand(&self) -> impl IntoElement {
        div()
            .px_4()
            .py_4()
            .child(div().text_lg().font_semibold().child("Einfach Mail"))
    }

    fn render_feature_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("workspace-feature-navigation")
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .child(self.render_navigation_button(WorkspaceSection::Mail, cx))
            .child(self.render_navigation_button(WorkspaceSection::Calendar, cx))
    }

    fn render_active_account_identity(
        profile: &AccountProfile,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let identity = div()
            .id("active-account-identity")
            .role(Role::Group)
            .aria_label(format!("Active account: {}", profile.label()))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_3()
            .child(Icon::new(IconName::User).size_4())
            .child(Self::render_active_account_details(profile, colors));
        #[cfg(test)]
        let identity = identity.test_support();
        identity
    }

    fn render_active_account_details(
        profile: &AccountProfile,
        colors: ColorTokens,
    ) -> impl IntoElement {
        let mailbox_address = profile
            .mailbox()
            .map(|mailbox| mailbox.address().to_owned())
            .filter(|address| !profile.label().eq_ignore_ascii_case(address));
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .font_semibold()
                    .truncate()
                    .child(profile.label().to_owned()),
            )
            .when_some(mailbox_address, |details, address| {
                details.child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .truncate()
                        .child(address),
                )
            })
    }

    fn render_account_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("workspace-account-navigation")
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .p_2()
            .child(self.render_navigation_button(WorkspaceSection::Accounts, cx))
    }

    fn render_navigation_button(
        &self,
        section: WorkspaceSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, label, icon) = section.navigation_identity();
        let selected = self.content.selected_section() == section;
        let disabled_reason = self.navigation_disabled_reason(section, cx);
        Button::new(id)
            .debug_selector(move || id.to_owned())
            .ghost()
            .selected(selected)
            .disabled(disabled_reason.is_some())
            .label(label)
            .icon(Icon::new(icon).size_4())
            .w_full()
            .track_focus(self.navigation_focus(section))
            .when_some(disabled_reason, |button, reason| button.tooltip(reason))
            .on_click(cx.listener(move |view, _, _, cx| view.navigate_to(section, cx)))
    }

    fn render_workspace_content(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match &self.content {
            AccountContent::Mail { view, .. } => {
                div().size_full().child(view.clone()).into_any_element()
            }
            AccountContent::Calendar { view, .. } => {
                div().size_full().child(view.clone()).into_any_element()
            }
            AccountContent::Account | AccountContent::Opening(_) => {
                self.render_account_workspace_content(colors, cx)
            }
        }
    }

    fn render_account_workspace_content(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        div()
            .size_full()
            .overflow_y_scrollbar()
            .id("account-scroll-viewport")
            .bg(colors.background)
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_8()
                    .child(self.render_account_content(colors, cx)),
            )
            .into_any_element()
    }
}

impl<P: AccountProvider + 'static> Render for AccountView<P> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id("workspace")
            .size_full()
            .flex()
            .child(self.render_navigation(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .overflow_hidden()
                    .child(self.render_workspace_content(colors, cx)),
            )
            .into_any_element()
    }
}

fn mail_access_summary(capabilities: MailCapabilities) -> String {
    let mut actions = vec!["read messages"];
    for (action, label) in [
        (MailAction::SetReadState, "mark read or unread"),
        (
            MailAction::SetFollowUpFlagStatus,
            "set or clear follow-up flags",
        ),
        (MailAction::Send, "send"),
        (MailAction::Reply, "reply"),
        (MailAction::Move, "move or archive"),
        (MailAction::Delete, "delete"),
    ] {
        if capabilities.supports(action) {
            actions.push(label);
        }
    }
    format!("Mail actions: {}.", actions.join(", "))
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

fn account_cache_open_error_message(error: AccountCacheOpenError) -> &'static str {
    match error {
        AccountCacheOpenError::InUse => {
            "Another Einfach Mail operation is opening or using this account cache. Wait for it to finish, or close the other account view and try again."
        }
        AccountCacheOpenError::MissingEncryptionKey => {
            "The encrypted account cache key is missing. Remove the local cache before syncing again."
        }
        AccountCacheOpenError::Unavailable => {
            "The encrypted account cache could not be opened. Check that the cache files are accessible and your Linux keyring is unlocked, then try again."
        }
        AccountCacheOpenError::InvalidData | AccountCacheOpenError::AccountMismatch => {
            "The account cache could not be opened. Try again."
        }
    }
}

fn account_cache_removal_error_message(error: AccountCacheRemovalError) -> &'static str {
    match error {
        AccountCacheRemovalError::InUse => {
            "Another Einfach Mail operation is opening or using this account cache. Wait for it to finish, or close the other account view and try again."
        }
        AccountCacheRemovalError::OperationInProgress => {
            "A mail or calendar operation is still active. Wait for it to finish, then try signing out again."
        }
        AccountCacheRemovalError::PendingMailSend => {
            "A previous send needs review. Open Mail, check Sent Items, and resolve it before signing out."
        }
        AccountCacheRemovalError::PendingCalendarEventCreation => {
            "A calendar event may already exist in Microsoft. Open Calendar and resolve the saved operation before signing out."
        }
        AccountCacheRemovalError::MissingEncryptionKey => {
            "The encrypted account cache key is missing. Remove the local cache before syncing again."
        }
        AccountCacheRemovalError::InvalidData | AccountCacheRemovalError::AccountMismatch => {
            "The account cache could not be verified for removal. Your account is still signed in."
        }
        AccountCacheRemovalError::Unavailable => {
            "The encrypted cache could not be read or removed. Check that the cache files are accessible and your Linux keyring is unlocked, then try again."
        }
    }
}

#[cfg(test)]
mod tests;
