use std::sync::Arc;

use crate::calendar_view::{CalendarView, CalendarViewEvent};
use crate::mail_composer::MailPermissionRequester;
use crate::mail_view::{MailView, MailViewEvent};
use einfach_mail_application::{
    AccountCacheOpenError, AccountCacheRemovalError, AccountOperationCancellation,
    AccountOperationError, AccountPermissionRequest, AccountProvider, AccountSession,
    AccountSessionFactory, AccountWorkflow,
};
use einfach_mail_domain::AccountProfile;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
};
use gpui_kit::{
    AsyncApp, Context, Entity, IntoElement, Render, Role, Subscription, Task, Window, div,
    prelude::*,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SetupIssue {
    MissingClientId,
    InvalidClientId,
    MissingCallbackPort,
    InvalidCallbackPort,
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
        }
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
    SigningOut,
    Connected(AccountProfile),
    Reauthorizing {
        profile: AccountProfile,
        cancellation: AccountOperationCancellation,
    },
    CancellingReauthorization {
        profile: AccountProfile,
    },
    ReauthorizationFailed {
        profile: AccountProfile,
        error: AccountOperationError,
    },
    SignOutFailed {
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
            Self::SigningOut
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
        match self {
            Self::Connected(profile)
            | Self::Reauthorizing { profile, .. }
            | Self::CancellingReauthorization { profile }
            | Self::ReauthorizationFailed { profile, .. } => Some(profile),
            _ => None,
        }
    }

    fn error_message(error: AccountOperationError) -> &'static str {
        match error {
            AccountOperationError::SignInCancelled => "Sign-in was cancelled.",
            AccountOperationError::AccessDenied => {
                "Microsoft did not grant access. Check your organization's consent policies."
            }
            AccountOperationError::AccessTokenRejected => {
                "Microsoft rejected the sign-in. Connect your account again."
            }
            AccountOperationError::SignInExpired => {
                "The saved sign-in has expired. Connect your account again."
            }
            AccountOperationError::CredentialStoreUnavailable => {
                "Linux Secret Service is unavailable. Unlock your keyring and try again."
            }
            AccountOperationError::ProfileUnavailable => {
                "Microsoft did not return the account details needed to identify this account."
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
    workflow: Option<Arc<AccountWorkflow<P>>>,
    account_session_factory: Option<Arc<dyn AccountSessionFactory>>,
    account_session: Option<Arc<dyn AccountSession>>,
    setup_issue: Option<SetupIssue>,
    status: AccountStatus,
    pending_permission_review: Option<AccountPermissionRequest>,
    content: AccountContent,
    session_open_error: Option<AccountViewCacheError>,
    task: Option<Task<()>>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum OpeningContent {
    Mail,
    Calendar,
}

enum AccountContent {
    Account,
    Opening(OpeningContent),
    Mail {
        view: Entity<MailView>,
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
        workflow: Result<Arc<AccountWorkflow<P>>, SetupIssue>,
        account_session_factory: Option<Arc<dyn AccountSessionFactory>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (workflow, setup_issue, status) = match workflow {
            Ok(workflow) => (Some(workflow), None, AccountStatus::Restoring),
            Err(issue) => (None, Some(issue), AccountStatus::Disconnected),
        };
        let mut view = Self {
            workflow,
            account_session_factory,
            account_session: None,
            setup_issue,
            status,
            pending_permission_review: None,
            content: AccountContent::Account,
            session_open_error: None,
            task: None,
        };
        view.start_restore(cx);
        view
    }

    fn start_restore(&mut self, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow.clone() else {
            return;
        };
        self.status = AccountStatus::Restoring;
        self.session_open_error = None;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let restore_result = workflow.restore_account().await;
            let _ = this.update(cx, |view, cx| {
                view.task = None;
                view.status = match restore_result {
                    Ok(Some(profile)) => AccountStatus::Connected(profile),
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
        let Some(workflow) = self.workflow.clone() else {
            return;
        };
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
            saved_credential_may_exist,
            cancellation,
            cx,
        ));
        cx.notify();
    }

    fn connect_task(
        workflow: Arc<AccountWorkflow<P>>,
        saved_credential_may_exist: bool,
        cancellation: AccountOperationCancellation,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let connection_result = workflow
                .connect_account_with_cancellation(cancellation)
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
                    Ok(profile) => AccountStatus::Connected(profile),
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
        let Some(workflow) = self.workflow.clone() else {
            return;
        };
        let Some(profile) = self.status.connected_profile().cloned() else {
            return;
        };
        let cancellation = AccountOperationCancellation::new();
        self.pending_permission_review = None;
        self.status = AccountStatus::Reauthorizing {
            profile: profile.clone(),
            cancellation: cancellation.clone(),
        };
        self.task = Some(Self::reauthorize_task(
            workflow,
            profile,
            request,
            cancellation,
            cx,
        ));
        cx.notify();
    }

    fn reauthorize_task(
        workflow: Arc<AccountWorkflow<P>>,
        profile: AccountProfile,
        request: AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let reauthorization_result = workflow
                .reauthorize_account_with_cancellation(profile.id(), request, cancellation)
                .await;
            let _ = this.update(cx, |view, cx| {
                view.task = None;
                if reauthorization_result.is_err() {
                    view.pending_permission_review = Some(request);
                }
                view.status = match reauthorization_result {
                    Ok(profile) => AccountStatus::Connected(profile),
                    Err(error) => AccountStatus::ReauthorizationFailed { profile, error },
                };
                cx.notify();
            });
        })
    }

    fn cancel_reauthorization(&mut self, cx: &mut Context<Self>) {
        let AccountStatus::Reauthorizing {
            profile,
            cancellation,
        } = &self.status
        else {
            return;
        };
        cancellation.cancel();
        self.status = AccountStatus::CancellingReauthorization {
            profile: profile.clone(),
        };
        cx.notify();
    }

    fn sign_out(&mut self, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow.clone() else {
            return;
        };
        if !self.status.saved_credential_may_exist()
            || self.content.opening_kind().is_some()
            || matches!(
                &self.status,
                AccountStatus::Connecting { .. }
                    | AccountStatus::CancellingConnect { .. }
                    | AccountStatus::Reauthorizing { .. }
                    | AccountStatus::CancellingReauthorization { .. }
                    | AccountStatus::SigningOut
            )
        {
            return;
        }

        let previous_status = self.status.clone();
        let account_session_factory = self.account_session_factory.clone();
        let account_session = self
            .close_content_view(cx)
            .or_else(|| self.account_session.clone());
        self.content = AccountContent::Account;
        self.session_open_error = None;
        self.status = AccountStatus::SigningOut;
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
            AccountContent::Mail { view, subscription } => {
                drop(subscription);
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
        let permission_requester = self
            .workflow
            .clone()
            .map(|workflow| -> Arc<dyn MailPermissionRequester> { workflow });
        self.open_account_content(
            OpeningContent::Mail,
            move |profile, session, cx| {
                let mail_view_session = session.mail_session();
                let mail_view = cx
                    .new(|cx| MailView::new(profile, mail_view_session, permission_requester, cx));
                let subscription = cx.subscribe(
                    &mail_view,
                    |view, _, event: &MailViewEvent, cx| match event {
                        MailViewEvent::BackToAccount => {
                            drop(view.close_content_view(cx));
                            cx.notify();
                        }
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
                    subscription,
                }
            },
            cx,
        );
    }

    fn open_calendar(&mut self, cx: &mut Context<Self>) {
        self.open_account_content(
            OpeningContent::Calendar,
            |profile, session, cx| {
                let calendar_session = session.calendar_session();
                let calendar_view = cx.new(|cx| CalendarView::new(profile, calendar_session, cx));
                let subscription =
                    cx.subscribe(&calendar_view, |view, _, event: &CalendarViewEvent, cx| {
                        match event {
                            CalendarViewEvent::BackToAccount => {
                                drop(view.close_content_view(cx));
                                cx.notify();
                            }
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
        let AccountStatus::Connected(profile) = &self.status else {
            return;
        };
        let Some(account_session_factory) = self.account_session_factory.clone() else {
            return;
        };
        let profile = profile.clone();
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
                            if current_profile.id() == profile.id()
                    )
                {
                    return;
                }
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
        account_session_factory: Option<Arc<dyn AccountSessionFactory>>,
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
                self.status = AccountStatus::SignOutFailed { error };
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
                "Waiting for Microsoft sign-in",
                "Waiting for Microsoft sign-in in your browser…",
                Role::Status,
            ),
            AccountStatus::CancellingConnect { .. } => (
                "Finishing Microsoft sign-in",
                "Finishing sign-in…",
                Role::Status,
            ),
            AccountStatus::SigningOut => (
                "Removing saved sign-in",
                "Removing saved sign-in…",
                Role::Status,
            ),
            AccountStatus::SignOutFailed { error } => (
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
            AccountStatus::Connected(profile) => format!("Connected to {}", profile.label()),
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
        let profile = self.status.connected_profile()?;
        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(self.account_identity(profile, colors))
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child("Signing out removes the saved sign-in and encrypted mail and calendar cache from this device. Microsoft organization consent is unchanged."),
                ),
        )
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

    fn render_primary_action(&self, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.status {
            AccountStatus::Restoring => Self::render_progress_action("Checking account…"),
            AccountStatus::SigningOut => Self::render_progress_action("Signing out…"),
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
            AccountStatus::Connected(_) if self.account_session_factory.is_some() => {
                self.render_open_content(cx)
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
            } => div()
                .flex()
                .justify_center()
                .debug_selector(|| "connect-account".to_owned())
                .child(
                    Button::new("connect-account")
                        .primary()
                        .label("Connect Microsoft 365 account")
                        .on_click(cx.listener(|view, _, _, cx| view.connect(cx))),
                ),
            AccountStatus::Connected(_) | AccountStatus::ReauthorizationFailed { .. } => div(),
        }
    }

    fn render_open_content(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let mail_opening = self.content.opening_kind() == Some(OpeningContent::Mail);
        let calendar_opening = self.content.opening_kind() == Some(OpeningContent::Calendar);
        let opening = self.content.opening_kind().is_some();
        div()
            .flex()
            .justify_center()
            .gap_2()
            .child(
                Button::new("open-mail")
                    .debug_selector(|| "open-mail".to_owned())
                    .primary()
                    .label(if mail_opening {
                        "Opening mail…"
                    } else {
                        "Open mail"
                    })
                    .loading(mail_opening)
                    .disabled(opening)
                    .on_click(cx.listener(|view, _, _, cx| view.open_mail(cx))),
            )
            .child(
                Button::new("open-calendar")
                    .debug_selector(|| "open-calendar".to_owned())
                    .outline()
                    .label(if calendar_opening {
                        "Opening calendar…"
                    } else {
                        "Open calendar"
                    })
                    .loading(calendar_opening)
                    .disabled(opening)
                    .on_click(cx.listener(|view, _, _, cx| view.open_calendar(cx))),
            )
    }

    fn render_header(&self, colors: ColorTokens) -> gpui_kit::Div {
        div()
            .w_full()
            .max_w_48()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_lg().font_bold().child("Accounts"))
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child("Connect a Microsoft 365 work account to Einfach Mail."),
            )
            .child(self.render_permissions(colors))
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
        if let Some(issue) = self.setup_issue {
            return content.child(self.render_setup_status(issue));
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
        if let Some(details) = self.render_account_details(colors) {
            content = content.child(details);
        }
        if matches!(
            &self.status,
            AccountStatus::Connected(_) | AccountStatus::ReauthorizationFailed { .. }
        ) {
            content = content.child(self.render_permission_review(cx));
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
            return content.child(self.render_primary_action(cx));
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
        content.child(self.render_primary_action(cx))
    }
}

impl<P: AccountProvider + 'static> Render for AccountView<P> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.content {
            AccountContent::Mail { view, .. } => {
                return div().size_full().child(view.clone()).into_any_element();
            }
            AccountContent::Calendar { view, .. } => {
                return div().size_full().child(view.clone()).into_any_element();
            }
            AccountContent::Account | AccountContent::Opening(_) => {}
        }
        let colors = cx.theme().semantic_tokens().colors;
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

async fn remove_account_cache(
    account_session: Option<Arc<dyn AccountSession>>,
    account_session_factory: Option<Arc<dyn AccountSessionFactory>>,
    account_id: einfach_mail_domain::AccountId,
) -> Result<(), AccountCacheRemovalError> {
    match account_session {
        Some(account_session) => account_session.remove_account_cache(&account_id).await,
        None => {
            account_session_factory
                .ok_or(AccountCacheRemovalError::Unavailable)?
                .remove_account_cache(&account_id)
                .await
        }
    }
}

async fn remove_verified_account_cache_and_sign_out<P: AccountProvider>(
    workflow: Arc<AccountWorkflow<P>>,
    account_session: Option<Arc<dyn AccountSession>>,
    account_session_factory: Option<Arc<dyn AccountSessionFactory>>,
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
            "The account cache could not be opened. Try again."
        }
        AccountCacheRemovalError::Unavailable => {
            "The encrypted cache could not be read or removed. Check that the cache files are accessible and your Linux keyring is unlocked, then try again."
        }
    }
}

#[cfg(test)]
mod tests;
