use std::sync::Arc;

use crate::calendar_view::{CalendarView, CalendarViewEvent};
use crate::mail_composer::MailPermissionRequester;
use crate::mail_view::{MailView, MailViewEvent};
use einfach_mail_application::{
    AccountOperationCancellation, AccountOperationError, AccountPermissionRequest, AccountProvider,
    AccountSession, AccountSessionFactory, AccountWorkflow, MailCacheError,
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
                "Microsoft returned a different account. Your saved sign-in was not changed."
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
    opening_content: Option<OpeningContent>,
    session_open_error: Option<MailCacheError>,
    mail_view: Option<Entity<MailView>>,
    mail_view_subscription: Option<Subscription>,
    calendar_view: Option<Entity<CalendarView>>,
    calendar_view_subscription: Option<Subscription>,
    task: Option<Task<()>>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum OpeningContent {
    Mail,
    Calendar,
}

fn permission_review_disclosure(request: AccountPermissionRequest) -> &'static str {
    match request {
        AccountPermissionRequest::Current => {
            "Microsoft will review the existing delegated access: User.Read, Mail.Read, Calendars.Read, and offline_access."
        }
        AccountPermissionRequest::MailReadWrite => {
            "Microsoft grants Mail.ReadWrite, which can create, read, update, and delete mail in this account. Einfach Mail requests it when Microsoft denies a read-state change, follow-up flag change, or message move. It uses the permission to set read state, update or clear follow-up flags, and move messages between folders. This permission review does not perform the action or send mail, and it does not request Mail.Send. Continue opens Microsoft in your system browser."
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
            opening_content: None,
            session_open_error: None,
            mail_view: None,
            mail_view_subscription: None,
            calendar_view: None,
            calendar_view_subscription: None,
            task: None,
        };
        view.start_restore(cx);
        view
    }

    fn start_restore(&mut self, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow.clone() else {
            return;
        };
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let restore_result = workflow.restore_account().await;
            let _ = this.update(cx, |view, cx| {
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
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow.clone() else {
            return;
        };
        if !matches!(
            &self.status,
            AccountStatus::Disconnected | AccountStatus::Failed { .. }
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
        if self.opening_content.is_some() || self.status.connected_profile().is_none() {
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
            || self.opening_content.is_some()
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

        let connected_account = self.account_session_factory.clone().and_then(|factory| {
            self.status
                .connected_profile()
                .map(|profile| (self.status.clone(), profile.clone(), factory))
        });
        let account_session = self
            .close_mail_view(cx)
            .or_else(|| self.close_calendar_view(cx))
            .or_else(|| self.account_session.clone());
        self.opening_content = None;
        self.session_open_error = None;
        self.status = AccountStatus::SigningOut;
        self.task = Some(Self::sign_out_task(
            workflow,
            connected_account,
            account_session,
            cx,
        ));
        cx.notify();
    }

    fn close_mail_view(&mut self, cx: &mut Context<Self>) -> Option<Arc<dyn AccountSession>> {
        let mail_view = self.mail_view.take()?;
        drop(self.mail_view_subscription.take());
        mail_view.update(cx, |view, _| view.stop_pending_work());
        self.account_session.clone()
    }

    fn close_calendar_view(&mut self, cx: &mut Context<Self>) -> Option<Arc<dyn AccountSession>> {
        let calendar_view = self.calendar_view.take()?;
        drop(self.calendar_view_subscription.take());
        calendar_view.update(cx, |view, _| view.stop_pending_work());
        self.account_session.clone()
    }

    fn open_mail(&mut self, cx: &mut Context<Self>) {
        if self.opening_content.is_some()
            || self.mail_view.is_some()
            || self.calendar_view.is_some()
        {
            return;
        }
        let AccountStatus::Connected(profile) = &self.status else {
            return;
        };
        let Some(account_session_factory) = self.account_session_factory.clone() else {
            return;
        };
        let permission_requester = self
            .workflow
            .clone()
            .map(|workflow| -> Arc<dyn MailPermissionRequester> { workflow });
        let profile = profile.clone();
        let retained_session = self.account_session.clone();
        self.opening_content = Some(OpeningContent::Mail);
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
            match session {
                Ok(session) => {
                    let _ = this.update(cx, |view, cx| {
                        if view.opening_content != Some(OpeningContent::Mail)
                            || !matches!(&view.status, AccountStatus::Connected(_))
                        {
                            return;
                        }
                        view.opening_content = None;
                        let mail_view_session = session.mail_session();
                        let mail_view = cx.new(|cx| {
                            MailView::new(profile, mail_view_session, permission_requester, cx)
                        });
                        let subscription =
                            cx.subscribe(&mail_view, |view, _, event: &MailViewEvent, cx| {
                                match event {
                                    MailViewEvent::BackToAccount => {
                                        drop(view.close_mail_view(cx));
                                        cx.notify();
                                    }
                                    MailViewEvent::ReviewMailWritePermission => {
                                        drop(view.close_mail_view(cx));
                                        view.prepare_permission_review(
                                            AccountPermissionRequest::MailReadWrite,
                                            cx,
                                        );
                                    }
                                }
                            });
                        view.mail_view = Some(mail_view);
                        view.mail_view_subscription = Some(subscription);
                        view.account_session = Some(session);
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |view, cx| {
                        view.opening_content = None;
                        view.session_open_error = Some(error);
                        cx.notify();
                    });
                }
            }
        }));
        cx.notify();
    }

    fn open_calendar(&mut self, cx: &mut Context<Self>) {
        if self.opening_content.is_some()
            || self.mail_view.is_some()
            || self.calendar_view.is_some()
        {
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
        self.opening_content = Some(OpeningContent::Calendar);
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
            match session {
                Ok(session) => {
                    let _ = this.update(cx, |view, cx| {
                        if view.opening_content != Some(OpeningContent::Calendar)
                            || !matches!(&view.status, AccountStatus::Connected(_))
                        {
                            return;
                        }
                        view.opening_content = None;
                        let calendar_session = session.calendar_session();
                        let calendar_view =
                            cx.new(|cx| CalendarView::new(profile, calendar_session, cx));
                        let subscription = cx.subscribe(
                            &calendar_view,
                            |view, _, event: &CalendarViewEvent, cx| match event {
                                CalendarViewEvent::BackToAccount => {
                                    drop(view.close_calendar_view(cx));
                                    cx.notify();
                                }
                                CalendarViewEvent::ReviewCalendarWritePermission => {
                                    drop(view.close_calendar_view(cx));
                                    view.prepare_permission_review(
                                        AccountPermissionRequest::CalendarReadWrite,
                                        cx,
                                    );
                                }
                            },
                        );
                        view.calendar_view = Some(calendar_view);
                        view.calendar_view_subscription = Some(subscription);
                        view.account_session = Some(session);
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |view, cx| {
                        view.opening_content = None;
                        view.session_open_error = Some(error);
                        cx.notify();
                    });
                }
            }
        }));
        cx.notify();
    }

    fn sign_out_task(
        workflow: Arc<AccountWorkflow<P>>,
        connected_account: Option<(
            AccountStatus,
            AccountProfile,
            Arc<dyn AccountSessionFactory>,
        )>,
        account_session: Option<Arc<dyn AccountSession>>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            if let Some((previous_status, connected_profile, account_session_factory)) =
                connected_account
            {
                let account_id = connected_profile.id().clone();
                let cache_removal = cx
                    .background_executor()
                    .spawn(remove_account_cache(
                        account_session,
                        account_session_factory,
                        account_id,
                    ))
                    .await;
                if let Err(error) = cache_removal {
                    let _ = this.update(cx, |view, cx| {
                        view.status = previous_status;
                        view.session_open_error = Some(error);
                        if !matches!(
                            error,
                            MailCacheError::OperationInProgress
                                | MailCacheError::SendAlreadyPending
                                | MailCacheError::CalendarEventCreationUnresolved
                        ) {
                            view.account_session = None;
                        }
                        view.task = None;
                        cx.notify();
                    });
                    return;
                }
            }
            let sign_out_result = workflow.sign_out().await;
            let _ = this.update(cx, |view, cx| {
                view.account_session = None;
                view.status = match sign_out_result {
                    Ok(()) => AccountStatus::Disconnected,
                    Err(error) => AccountStatus::Failed {
                        error,
                        saved_credential_may_exist: true,
                    },
                };
                cx.notify();
            });
        })
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
                    .child("Initial sign-in requests User.Read, Mail.Read, Calendars.Read, and offline_access. Mail.Read lets Einfach Mail read your mail, and Calendars.Read lets it read your primary calendar. If Microsoft denies a read-state change, follow-up flag change, or message move, a separate review can request Mail.ReadWrite, which permits creating, reading, updating, and deleting mail. The app uses this access to set messages read or unread, update or clear follow-up flags, and move messages between folders. If Microsoft denies a supported calendar change, a separate review can request Calendars.ReadWrite, which permits creating, reading, updating, and deleting calendar events. Einfach Mail uses it to create one-off timed appointments and update the title or time of standalone appointments that you organize and that have no attendees. It does not create recurring or all-day events, send invitations, create online meetings, or delete events. The app does not request Mail.Send."),
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
                    .disabled(self.opening_content.is_some())
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
                                .disabled(self.task.is_some() || self.opening_content.is_some())
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
                        .disabled(self.opening_content.is_some() || self.task.is_some())
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
            AccountStatus::Disconnected | AccountStatus::Failed { .. } => div()
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
        let mail_opening = self.opening_content == Some(OpeningContent::Mail);
        let calendar_opening = self.opening_content == Some(OpeningContent::Calendar);
        let opening = self.opening_content.is_some();
        div()
            .flex()
            .justify_center()
            .gap_2()
            .child(
                Button::new("open-mail")
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
            content = content.child(
                div()
                    .id("account-cache-error")
                    .role(Role::Alert)
                    .text_sm()
                    .child(mail_cache_error_message(error)),
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
        if let Some(mail_view) = &self.mail_view {
            return div()
                .size_full()
                .child(mail_view.clone())
                .into_any_element();
        }
        if let Some(calendar_view) = &self.calendar_view {
            return div()
                .size_full()
                .child(calendar_view.clone())
                .into_any_element();
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
    account_session_factory: Arc<dyn AccountSessionFactory>,
    account_id: einfach_mail_domain::AccountId,
) -> Result<(), MailCacheError> {
    match account_session {
        Some(account_session) => account_session.remove_account_cache(&account_id).await,
        None => {
            account_session_factory
                .remove_account_cache(&account_id)
                .await
        }
    }
}

fn mail_cache_error_message(error: MailCacheError) -> &'static str {
    match error {
        MailCacheError::OperationInProgress => {
            "A mail or calendar operation is still active. Wait for it to finish, then try signing out again."
        }
        MailCacheError::SendAlreadyPending => {
            "A previous send needs review. Open Mail, check Sent Items, and resolve it before signing out."
        }
        MailCacheError::CalendarEventCreationUnresolved => {
            "A calendar event may already exist in Microsoft. Open Calendar and resolve the saved operation before signing out."
        }
        MailCacheError::MissingEncryptionKey => {
            "The encrypted account cache key is missing. Remove the local cache before syncing again."
        }
        MailCacheError::Unavailable => {
            "The encrypted cache could not be read or removed. Check that the cache files are accessible and your Linux keyring is unlocked, then try again."
        }
        _ => "The local mail cache could not be opened. Try again.",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::{Future, ready},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Duration,
    };

    use tokio::sync::oneshot;

    use gpui_kit::{
        AppContext as _, Modifiers, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext,
        point, px, size,
    };

    use super::{AccountStatus, AccountView, SetupIssue};
    use einfach_mail_application::{
        AccountOperationCancellation, AccountOperationError, AccountPermissionRequest,
        AccountProvider, AccountSession, AccountSessionFactory, AccountSessionFuture,
        AccountWorkflow, CalendarCacheError, CalendarEventCreationOutcome,
        CalendarEventUpdateOutcome, CalendarSession, CalendarSnapshot, CalendarSyncReport,
        CalendarWorkflowError, MailCacheError, MailSession,
    };
    use einfach_mail_domain::{
        AccountId, AccountProfile, CalendarEventCreation, CalendarEventUpdate, CalendarOccurrence,
        CalendarViewReference, EditableCalendarEvent,
    };

    #[derive(Clone, Default)]
    struct TestProvider {
        signed_out: Arc<AtomicBool>,
        fail_next_sign_out: Arc<AtomicBool>,
        pause_connection: Arc<AtomicBool>,
        pause_permission_update: Arc<AtomicBool>,
        connection_commit: Arc<Mutex<Option<CommitGate>>>,
        permission_commit: Arc<Mutex<Option<CommitGate>>>,
        last_permission_request: Arc<Mutex<Option<AccountPermissionRequest>>>,
    }

    struct CommitGate {
        started: mpsc::Sender<()>,
        finish: oneshot::Receiver<()>,
    }

    struct TestAccountSession {
        removal_attempts: Arc<Mutex<Vec<AccountId>>>,
    }

    impl AccountSession for TestAccountSession {
        fn mail_session(&self) -> Arc<dyn MailSession> {
            panic!("the retained test session is not reopened")
        }

        fn calendar_session(&self) -> Arc<dyn CalendarSession> {
            Arc::new(TestCalendarSession)
        }

        fn remove_account_cache<'a>(
            &'a self,
            account_id: &'a AccountId,
        ) -> AccountSessionFuture<'a, Result<(), MailCacheError>> {
            let attempts = Arc::clone(&self.removal_attempts);
            let account_id = account_id.clone();
            Box::pin(async move {
                let mut attempts = attempts
                    .lock()
                    .expect("synthetic session removal log mutex is healthy");
                attempts.push(account_id);
                if attempts.len() == 1 {
                    Err(MailCacheError::OperationInProgress)
                } else {
                    Ok(())
                }
            })
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
        ) -> AccountSessionFuture<'a, Result<EditableCalendarEvent, CalendarWorkflowError>>
        {
            Box::pin(ready(Err(CalendarWorkflowError::Cache(
                CalendarCacheError::Unavailable,
            ))))
        }

        fn create_event<'a>(
            &'a self,
            _: &'a CalendarEventCreation,
        ) -> AccountSessionFuture<'a, Result<CalendarEventCreationOutcome, CalendarWorkflowError>>
        {
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
        ) -> AccountSessionFuture<'a, Result<CalendarEventUpdateOutcome, CalendarWorkflowError>>
        {
            Box::pin(ready(Err(CalendarWorkflowError::Cache(
                CalendarCacheError::Unavailable,
            ))))
        }
    }

    struct TestAccountSessionFactory {
        session: Arc<dyn AccountSession>,
        fallback_removals: Arc<AtomicBool>,
    }

    impl AccountSessionFactory for TestAccountSessionFactory {
        fn open_session(
            &self,
            _: AccountId,
        ) -> AccountSessionFuture<'_, Result<Arc<dyn AccountSession>, MailCacheError>> {
            Box::pin(ready(Ok(Arc::clone(&self.session))))
        }

        fn remove_account_cache<'a>(
            &'a self,
            _: &'a AccountId,
        ) -> AccountSessionFuture<'a, Result<(), MailCacheError>> {
            self.fallback_removals.store(true, Ordering::SeqCst);
            Box::pin(ready(Err(MailCacheError::Unavailable)))
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
            ready(Ok(None))
        }

        fn connect_account(
            &self,
            cancellation: AccountOperationCancellation,
        ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_
        {
            let pause_connection = Arc::clone(&self.pause_connection);
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
            async move {
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
                Ok(profile)
            }
        }

        fn reauthorize_account(
            &self,
            expected_account_id: &AccountId,
            request: AccountPermissionRequest,
            cancellation: AccountOperationCancellation,
        ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_
        {
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
    fn unavailable_cache_message_covers_file_and_keyring_recovery() {
        let message = super::mail_cache_error_message(MailCacheError::Unavailable);

        assert!(message.contains("cache files"));
        assert!(message.contains("Linux keyring"));
        assert!(message.contains("try again"));
    }

    #[gpui::test]
    fn connects_and_signs_out_through_the_account_view(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let provider = TestProvider::default();
        let signed_out = provider.signed_out.clone();
        provider.fail_next_sign_out.store(true, Ordering::SeqCst);
        let workflow = Arc::new(AccountWorkflow::new(provider));
        let (view, context) = cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), None, cx));

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
                super::AccountStatus::Failed {
                    error: AccountOperationError::CredentialStoreUnavailable,
                    saved_credential_may_exist: true,
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
    }

    #[gpui::test]
    fn sign_out_after_calendar_return_reuses_the_retained_session(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let provider = TestProvider::default();
        let signed_out = Arc::clone(&provider.signed_out);
        let workflow = Arc::new(AccountWorkflow::new(provider));
        let removal_attempts = Arc::new(Mutex::new(Vec::new()));
        let fallback_removals = Arc::new(AtomicBool::new(false));
        let session: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
            removal_attempts: Arc::clone(&removal_attempts),
        });
        let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
            session: Arc::clone(&session),
            fallback_removals: Arc::clone(&fallback_removals),
        });
        let (view, context) =
            cx.add_window_view(|_, cx| AccountView::new(Ok(workflow), Some(factory), cx));
        context.run_until_parked();
        let profile = AccountProfile::try_new(
            AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is valid"),
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
            .update(|cx| view.read(cx).calendar_view.is_some());
        assert!(calendar_open);

        click(context, "calendar-back");
        let returned_to_account = context
            .cx
            .update(|cx| view.read(cx).calendar_view.is_none());
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
            view.task.is_none() && view.opening_content.is_none()
        });
        assert!(can_continue);
        let disclosure =
            super::permission_review_disclosure(AccountPermissionRequest::MailReadWrite);
        assert!(disclosure.contains("create, read, update, and delete mail"));
        assert!(disclosure.contains("move messages between folders"));
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

    #[gpui::test]
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
}
