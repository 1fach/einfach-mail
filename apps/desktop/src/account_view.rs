use std::sync::Arc;

use crate::calendar_view::{CalendarView, CalendarViewEvent};
use crate::mail_view::{MailView, MailViewEvent};
use einfach_mail_application::{
    AccountOperationCancellation, AccountOperationError, AccountProvider, AccountSession,
    AccountSessionFactory, AccountWorkflow, MailCacheError,
};
use einfach_mail_domain::AccountProfile;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
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

    fn reauthorize(&mut self, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow.clone() else {
            return;
        };
        let Some(profile) = self.status.connected_profile().cloned() else {
            return;
        };
        let cancellation = AccountOperationCancellation::new();
        self.status = AccountStatus::Reauthorizing {
            profile: profile.clone(),
            cancellation: cancellation.clone(),
        };
        self.task = Some(Self::reauthorize_task(workflow, profile, cancellation, cx));
        cx.notify();
    }

    fn reauthorize_task(
        workflow: Arc<AccountWorkflow<P>>,
        profile: AccountProfile,
        cancellation: AccountOperationCancellation,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let reauthorization_result = workflow
                .reauthorize_account_with_cancellation(profile.id(), cancellation)
                .await;
            let _ = this.update(cx, |view, cx| {
                view.task = None;
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
            .or_else(|| self.close_calendar_view(cx));
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
        self.account_session.take()
    }

    fn close_calendar_view(&mut self, cx: &mut Context<Self>) -> Option<Arc<dyn AccountSession>> {
        let calendar_view = self.calendar_view.take()?;
        drop(self.calendar_view_subscription.take());
        calendar_view.update(cx, |view, _| view.stop_pending_work());
        self.account_session.take()
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
        let profile = profile.clone();
        self.opening_content = Some(OpeningContent::Mail);
        self.session_open_error = None;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let account_id = profile.id().clone();
            let session = cx
                .background_executor()
                .spawn(async move { account_session_factory.open_session(account_id).await })
                .await;
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
                        let mail_view = cx.new(|cx| MailView::new(profile, mail_view_session, cx));
                        let subscription =
                            cx.subscribe(&mail_view, |view, _, event: &MailViewEvent, cx| {
                                match event {
                                    MailViewEvent::BackToAccount => {
                                        drop(view.close_mail_view(cx));
                                        cx.notify();
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
        self.opening_content = Some(OpeningContent::Calendar);
        self.session_open_error = None;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let account_id = profile.id().clone();
            let session = cx
                .background_executor()
                .spawn(async move { account_session_factory.open_session(account_id).await })
                .await;
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
                        view.task = None;
                        cx.notify();
                    });
                    return;
                }
            }
            let sign_out_result = workflow.sign_out().await;
            let _ = this.update(cx, |view, cx| {
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
                    .child("User.Read lets Einfach Mail display your account identity. Mail.Read lets it read your mail. Calendars.Read lets it read your primary calendar. offline_access lets it refresh your sign-in. Einfach Mail cannot send mail or change calendar events."),
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
        div()
            .flex()
            .justify_center()
            .debug_selector(|| "review-app-permissions".to_owned())
            .child(
                Button::new("review-app-permissions")
                    .label("Review sign-in permissions")
                    .disabled(self.opening_content.is_some())
                    .on_click(cx.listener(|view, _, _, cx| view.reauthorize(cx))),
            )
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
            content = content
                .child(self.render_permission_review(cx))
                .child(self.render_saved_sign_out(cx, "Sign out"));
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
            .flex()
            .items_center()
            .justify_center()
            .p_8()
            .bg(colors.background)
            .child(self.render_account_content(colors, cx))
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

    use gpui_kit::{AppContext as _, Modifiers, TestAppContext, VisualTestContext};

    use super::{AccountStatus, AccountView, SetupIssue};
    use einfach_mail_application::{
        AccountOperationCancellation, AccountOperationError, AccountProvider, AccountWorkflow,
        MailCacheError,
    };
    use einfach_mail_domain::{AccountId, AccountProfile};

    #[derive(Clone, Default)]
    struct TestProvider {
        signed_out: Arc<AtomicBool>,
        fail_next_sign_out: Arc<AtomicBool>,
        pause_connection: Arc<AtomicBool>,
        pause_permission_update: Arc<AtomicBool>,
        connection_commit: Arc<Mutex<Option<CommitGate>>>,
        permission_commit: Arc<Mutex<Option<CommitGate>>>,
    }

    struct CommitGate {
        started: mpsc::Sender<()>,
        finish: oneshot::Receiver<()>,
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
            cancellation: AccountOperationCancellation,
        ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_
        {
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

    fn click(context: &mut VisualTestContext, selector: &'static str) {
        let bounds = context
            .debug_bounds(selector)
            .expect("requested account action is rendered");
        context.simulate_click(bounds.center(), Modifiers::default());
        context.run_until_parked();
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
        let connect_bounds = context
            .debug_bounds("connect-account")
            .expect("connect button is rendered");
        context.simulate_click(connect_bounds.center(), Modifiers::default());
        context.run_until_parked();
        draw_account_view(context);
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
        click(context, "review-app-permissions");
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
