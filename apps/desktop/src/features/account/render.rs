use super::copy::{
    account_cache_removal_error_message, calendar_actions_summary, mail_access_summary,
    permission_review_disclosure,
};
use super::state::{
    AccountStatus, EwsDiscoveryState, ExchangeEwsConnectionForm, SavedCredentialState,
};
use super::{AccountView, SetupIssue};
use crate::components::{AppIcon, InfoBar, Page, SettingsCard, StatusTone, status_colors};
use einfach_mail_application::{
    AccountOperationError, AccountPermissionRequest, AccountProvider, AccountProviderKind,
    ConnectedAccount, ExchangeEwsDiscoveryConfirmation,
};
use einfach_mail_domain::AccountProfile;
use gpui_kit::component::{
    ColorTokens, Disableable, Icon,
    avatar::Avatar,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    h_flex,
    input::Input,
    v_flex,
};
use gpui_kit::{AnyElement, Context, Div, IntoElement, Role, Stateful, div, prelude::*};

impl<P: AccountProvider + 'static> AccountView<P> {
    fn account_identity(&self, profile: &AccountProfile, cx: &Context<Self>) -> SettingsCard {
        let connected = self.render_connected_status(cx);
        let mailbox = profile
            .mailbox()
            .map(|mailbox| div().child(format!("Mailbox: {}", mailbox.address())));
        let user_principal_name = profile
            .user_principal_name()
            .filter(|upn| !profile.label().eq_ignore_ascii_case(upn))
            .map(|upn| div().child(format!("Sign-in: {upn}")));
        SettingsCard::new("connected-account", profile.label().to_owned())
            .leading(Avatar::new().name(profile.label().to_owned()))
            .description(
                v_flex()
                    .gap_1()
                    .children(connected)
                    .children(mailbox)
                    .children(user_principal_name),
            )
    }

    fn render_connected_status(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let AccountStatus::Connected(account) = &self.status else {
            return None;
        };
        let success = status_colors(StatusTone::Success, cx).icon;
        Some(
            h_flex()
                .id("account-status")
                .role(Role::Status)
                .aria_label(format!("Connected to {}", account.profile().label()))
                .gap_1()
                .items_center()
                .child(Icon::new(AppIcon::Success).size_3().text_color(success))
                .child("Connected"),
        )
    }

    fn render_requested_access() -> SettingsCard {
        SettingsCard::new("account-requested-access", "Requested access")
            .icon(AppIcon::Permissions)
            .child("Initial sign-in requests User.Read, Mail.Read, Calendars.Read, and offline_access. Mail.Read lets Einfach Mail read your mail, and Calendars.Read lets it read your primary calendar. If Microsoft denies a read-state change, follow-up flag change, message move, or message deletion, a separate review can request Mail.ReadWrite, which permits creating, reading, updating, and deleting mail. The app uses this access to set messages read or unread, update or clear follow-up flags, move messages between folders, and delete selected messages after confirmation. If Microsoft denies a supported calendar change, a separate review can request Calendars.ReadWrite, which permits creating, reading, updating, and deleting calendar events. Einfach Mail uses it to create one-off timed appointments, update the title or time of supported standalone appointments you organize that have no attendees, and delete those appointments after confirmation. It does not create recurring or all-day events, send invitations, or create online meetings. The app does not request Mail.Send.")
    }

    fn render_status(&self) -> Option<InfoBar> {
        let (label, message, alert) = match &self.status {
            AccountStatus::Restoring => (
                "Checking for a saved account",
                "Checking for a saved account…",
                false,
            ),
            AccountStatus::Disconnected => ("Not connected", "No account is connected.", false),
            AccountStatus::Connecting { .. } => {
                ("Connecting to account", "Connecting to account…", false)
            }
            AccountStatus::CancellingConnect => (
                "Finishing account connection",
                "Finishing connection…",
                false,
            ),
            AccountStatus::SigningOut { .. } => {
                ("Removing saved sign-in", "Removing saved sign-in…", false)
            }
            AccountStatus::SignOutFailed { error, .. } => (
                AccountStatus::error_message(*error),
                AccountStatus::error_message(*error),
                true,
            ),
            AccountStatus::Reauthorizing { .. } => (
                "Reviewing Microsoft permissions",
                "Waiting for Microsoft permission review in your browser…",
                false,
            ),
            AccountStatus::CancellingReauthorization { .. } => (
                "Finishing Microsoft permission review",
                "Finishing permission review…",
                false,
            ),
            AccountStatus::ReauthorizationFailed { error, .. } => (
                AccountStatus::reauthorization_error_message(*error),
                AccountStatus::reauthorization_error_message(*error),
                true,
            ),
            AccountStatus::Connected(_) => return None,
            AccountStatus::Failed { error, .. } => (
                AccountStatus::error_message(*error),
                AccountStatus::error_message(*error),
                true,
            ),
        };
        let bar = if alert {
            InfoBar::alert("account-status", message)
        } else {
            InfoBar::status("account-status", message)
        };
        Some(bar.accessible_label(label))
    }

    /// A row that is as wide as its button, so the row's bounds are the button's bounds.
    fn action_slot() -> Div {
        div().flex().self_start()
    }

    fn render_account_details(
        &self,
        sign_out_label: Option<&'static str>,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let account = self.status.connected_account()?;
        let profile = account.profile();
        let mut identity = self.account_identity(profile, cx);
        if let Some(label) = sign_out_label {
            identity = identity.trailing(self.render_saved_sign_out(cx, label));
        }
        let identity = Self::with_account_detail_rows(identity, account, colors);
        let can_review_permissions = matches!(
            &self.status,
            AccountStatus::Connected(_) | AccountStatus::ReauthorizationFailed { .. }
        ) && profile.capabilities().can_review_mail_write_permission();
        let permissions = can_review_permissions.then(|| self.render_permission_review(cx));
        Some(v_flex().gap_1().child(identity).children(permissions))
    }

    fn with_account_detail_rows(
        identity: SettingsCard,
        account: &ConnectedAccount,
        colors: ColorTokens,
    ) -> SettingsCard {
        let capabilities = account.profile().capabilities();
        let provider = match account.provider_kind() {
            AccountProviderKind::MicrosoftGraph => "Microsoft Graph (Microsoft 365)",
            AccountProviderKind::ExchangeEws => "Exchange Web Services (Exchange Server)",
            _ => "Other provider",
        };
        identity
            .child(
                div()
                    .id("account-provider")
                    .debug_selector(|| "account-provider".to_owned())
                    .child(format!("Provider: {provider}")),
            )
            .child(
                div()
                    .id("account-mail-capabilities")
                    .debug_selector(|| "account-mail-capabilities".to_owned())
                    .child(mail_access_summary(capabilities.mail())),
            )
            .child(
                div()
                    .id("account-calendar-capabilities")
                    .debug_selector(|| "account-calendar-capabilities".to_owned())
                    .child(calendar_actions_summary(capabilities.calendar())),
            )
            .child(
                div()
                    .text_color(colors.muted_foreground)
                    .child("Signing out removes the saved sign-in and encrypted account cache from this device."),
            )
    }

    fn sign_out_label(&self) -> Option<&'static str> {
        match &self.status {
            AccountStatus::Connected(_) | AccountStatus::ReauthorizationFailed { .. } => self
                .pending_permission_review
                .is_none()
                .then_some("Sign out"),
            AccountStatus::SignOutFailed { .. } => Some("Retry sign out"),
            AccountStatus::Failed {
                saved_credential_state: SavedCredentialState::MayExist,
                error:
                    AccountOperationError::SavedAccountIdentityUnavailable
                    | AccountOperationError::AccountMismatch,
                ..
            } => None,
            AccountStatus::Failed {
                saved_credential_state: SavedCredentialState::MayExist,
                ..
            } => Some("Remove saved sign-in"),
            _ => None,
        }
    }

    fn render_saved_sign_out(
        &self,
        cx: &mut Context<Self>,
        label: &'static str,
    ) -> impl IntoElement {
        Self::action_slot()
            .debug_selector(|| "sign-out".to_owned())
            .child(
                Button::new("sign-out")
                    .danger()
                    .label(label)
                    .disabled(self.workspace_is_opening)
                    .on_click(cx.listener(|view, _, _, cx| view.request_sign_out(cx))),
            )
    }

    fn render_permission_review(&self, cx: &mut Context<Self>) -> SettingsCard {
        let card =
            SettingsCard::new("account-permissions", "Permissions").icon(AppIcon::Permissions);
        let Some(request) = self.pending_permission_review else {
            return card.trailing(
                Button::new("review-app-permissions")
                    .debug_selector(|| "review-app-permissions".to_owned())
                    .label("Review sign-in permissions")
                    .disabled(self.workspace_is_opening || self.task.is_some())
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.prepare_permission_review(AccountPermissionRequest::Current, cx);
                    })),
            );
        };
        card.child(
            v_flex()
                .gap_3()
                .id("permission-review-disclosure")
                .debug_selector(|| "permission-review-disclosure".to_owned())
                .role(Role::Alert)
                .child(permission_review_disclosure(request))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("continue-permission-review")
                                .debug_selector(|| "continue-permission-review".to_owned())
                                .primary()
                                .label("Continue to Microsoft")
                                .disabled(self.task.is_some() || self.workspace_is_opening)
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
                ),
        )
    }

    fn render_progress_action(label: &'static str) -> AnyElement {
        Self::action_slot()
            .child(
                Button::new("account-operation-progress")
                    .debug_selector(|| "account-operation-progress".to_owned())
                    .label(label)
                    .loading(true)
                    .disabled(true),
            )
            .into_any_element()
    }

    fn render_primary_action(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match &self.status {
            AccountStatus::Restoring => Some(Self::render_progress_action("Checking account…")),
            AccountStatus::SigningOut { .. } => Some(Self::render_progress_action("Signing out…")),
            AccountStatus::CancellingConnect => {
                Some(Self::render_progress_action("Finishing sign-in…"))
            }
            AccountStatus::CancellingReauthorization { .. } => {
                Some(Self::render_progress_action("Finishing permission review…"))
            }
            AccountStatus::Reauthorizing { .. } => Some(
                Self::action_slot()
                    .debug_selector(|| "cancel-permission-update".to_owned())
                    .child(
                        Button::new("cancel-permission-update")
                            .ghost()
                            .label("Cancel permission review")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.cancel_reauthorization(cx);
                            })),
                    )
                    .into_any_element(),
            ),
            AccountStatus::Connecting { .. } => Some(
                Self::action_slot()
                    .debug_selector(|| "cancel-sign-in".to_owned())
                    .child(
                        Button::new("cancel-sign-in")
                            .ghost()
                            .label("Cancel sign-in")
                            .on_click(cx.listener(|view, _, _, cx| view.cancel_connect(cx))),
                    )
                    .into_any_element(),
            ),
            AccountStatus::Failed {
                saved_credential_state: SavedCredentialState::MayExist,
                ..
            } => Some(
                Self::action_slot()
                    .debug_selector(|| "retry-account-restore".to_owned())
                    .child(
                        Button::new("retry-account-restore")
                            .primary()
                            .label("Retry account restore")
                            .on_click(cx.listener(|view, _, _, cx| view.start_restore(cx))),
                    )
                    .into_any_element(),
            ),
            AccountStatus::Disconnected
            | AccountStatus::Failed {
                saved_credential_state: SavedCredentialState::Absent,
                ..
            } => Some(self.render_connect_options(cx).into_any_element()),
            AccountStatus::SignOutFailed { .. }
            | AccountStatus::Connected(_)
            | AccountStatus::ReauthorizationFailed { .. } => None,
        }
    }

    fn render_connect_options(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .w_full()
            .gap_6()
            .child(self.render_microsoft365_choice(cx))
            .child(self.render_exchange_ews_choice(cx))
    }

    fn render_microsoft365_choice(&self, cx: &mut Context<Self>) -> Div {
        let card = SettingsCard::new("microsoft365-provider", "Microsoft 365")
            .icon(AppIcon::Mail)
            .description(Self::render_microsoft365_provider_description());
        let group = v_flex().gap_1();
        match self.microsoft365_setup_issue() {
            Some(issue) => group
                .child(card)
                .child(Self::render_microsoft365_setup_status(issue)),
            None => group
                .child(card.trailing(self.render_microsoft365_connect_button(cx)))
                .child(Self::render_requested_access()),
        }
    }

    fn microsoft365_setup_issue(&self) -> Option<SetupIssue> {
        self.services
            .as_ref()
            .ok()
            .and_then(|services| services.microsoft365_setup_issue)
    }

    fn render_microsoft365_provider_description() -> Stateful<Div> {
        div()
            .id("microsoft365-provider-description")
            .debug_selector(|| "microsoft365-provider-description".to_owned())
            .child(
                "Connect a work or school Microsoft 365 account with Microsoft Graph through the system browser.",
            )
    }

    fn render_exchange_ews_choice(&self, cx: &mut Context<Self>) -> SettingsCard {
        SettingsCard::new("exchange-ews-provider", "Exchange Server")
            .icon(AppIcon::Server)
            .description(Self::render_exchange_ews_provider_description())
            .trailing(self.render_ews_connect_button(cx))
    }

    fn render_exchange_ews_provider_description() -> Stateful<Div> {
        div()
            .id("exchange-ews-provider-description")
            .debug_selector(|| "exchange-ews-provider-description".to_owned())
            .child(
                "Connect to an on-premises Exchange Server with EWS. Enter an HTTPS endpoint or discover one from your mailbox.",
            )
    }

    fn render_microsoft365_setup_status(issue: SetupIssue) -> InfoBar {
        InfoBar::status("microsoft365-setup-status", issue.message()).tone(StatusTone::Warning)
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

    fn render_exchange_ews_form(
        &self,
        form: &ExchangeEwsConnectionForm,
        cx: &mut Context<Self>,
    ) -> SettingsCard {
        SettingsCard::new("exchange-ews-form", "Exchange Server")
            .icon(AppIcon::Server)
            .child(
                "Enter your server's HTTPS EWS address or discover it from your mailbox. Discovery sends the mailbox address without credentials. Review the endpoint before connecting; your NTLM username and password are sent only after you press Connect.",
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
                .children(self.render_ews_discovery_feedback(&form.discovery, cx)),
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
        div().flex().child(
            Button::new("discover-ews-endpoint")
                .debug_selector(|| "discover-ews-endpoint".to_owned())
                .outline()
                .label(form.discovery.button_label())
                .loading(form.discovery.is_busy())
                .disabled(self.ews_discovery_button_disabled(&form.discovery))
                .on_click(cx.listener(|view, _, window, cx| {
                    view.discover_exchange_ews_endpoint(window, cx);
                })),
        )
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
    ) -> Option<AnyElement> {
        if let EwsDiscoveryState::Confirmation(confirmation) = state {
            return Some(
                self.render_ews_discovery_confirmation(confirmation, cx)
                    .into_any_element(),
            );
        }
        let tone = match state {
            EwsDiscoveryState::Found => StatusTone::Success,
            EwsDiscoveryState::Failed => StatusTone::Warning,
            _ => StatusTone::Informational,
        };
        state.feedback_message().map(|message| {
            InfoBar::status("ews-discovery-status", message)
                .tone(tone)
                .into_any_element()
        })
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

    fn render_ews_confirmation_prompt(confirmation: &ExchangeEwsDiscoveryConfirmation) -> InfoBar {
        InfoBar::status(
            "ews-discovery-confirmation-status",
            format!(
                "Send {} to {} to continue Autodiscover?",
                confirmation.mailbox_address(),
                confirmation.destination()
            ),
        )
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
            Self::render_ews_identity_field(
                "ews-mailbox-address-input",
                "Mailbox address",
                Input::new(&form.mailbox_address)
                    .aria_label("Mailbox address")
                    .disabled(inputs_disabled)
                    .w_full(),
            ),
            Self::render_ews_identity_field(
                "ews-username-input",
                "NTLM username",
                Input::new(&form.username)
                    .aria_label("NTLM username")
                    .disabled(inputs_disabled)
                    .w_full(),
            ),
            Self::render_ews_identity_field(
                "ews-password-input",
                "Password",
                Input::new(&form.password)
                    .aria_label("Password")
                    .mask_toggle()
                    .disabled(inputs_disabled)
                    .w_full(),
            ),
        ]
    }

    fn render_ews_identity_field(
        id: &'static str,
        label: &'static str,
        input: impl IntoElement,
    ) -> Field {
        Field::new().label(label).required(true).child(
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .child(input),
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
        let cancelling = matches!(&self.status, AccountStatus::CancellingConnect);
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
        if matches!(&self.status, AccountStatus::CancellingConnect) {
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
            AccountStatus::Connecting { .. } | AccountStatus::CancellingConnect
        )
    }

    pub(super) fn account_header_description(&self) -> &'static str {
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
                saved_credential_state: SavedCredentialState::MayExist,
                error:
                    AccountOperationError::SavedAccountIdentityUnavailable
                    | AccountOperationError::AccountMismatch,
                ..
            } => Some("Retry account restore before removing or replacing the saved sign-in."),
            AccountStatus::Failed {
                saved_credential_state: SavedCredentialState::MayExist,
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
            AccountStatus::CancellingConnect => {
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

    fn render_setup_status(&self, issue: SetupIssue) -> InfoBar {
        let message = issue.message();
        InfoBar::status("account-status", message)
            .tone(StatusTone::Warning)
            .accessible_label(message)
    }

    fn render_account_content(&self, colors: ColorTokens, cx: &mut Context<Self>) -> Page {
        let page = Page::new("account-scroll-viewport", "Accounts")
            .description(self.account_header_description());
        if let Err(issue) = &self.services {
            return page.child(self.render_setup_status(*issue));
        }
        let cache_removal_error = self.cache_removal_error.map(|error| {
            InfoBar::alert(
                "account-cache-error",
                account_cache_removal_error_message(error),
            )
        });
        let page = page
            .children(self.render_status())
            .children(cache_removal_error);
        self.render_account_actions(page, colors, cx)
    }

    fn render_account_actions(
        &self,
        page: Page,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> Page {
        if let Some(form) = &self.exchange_ews_form {
            return page.child(self.render_exchange_ews_form(form, cx));
        }
        let sign_out_label = self.sign_out_label();
        let details = self.render_account_details(sign_out_label, colors, cx);
        let has_details = details.is_some();
        let standalone_sign_out = sign_out_label
            .filter(|_| !has_details)
            .map(|label| self.render_saved_sign_out(cx, label));
        page.children(details)
            .children(standalone_sign_out)
            .children(self.render_primary_action(cx))
    }

    pub(super) fn render_account_feature_content(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        self.render_account_content(colors, cx).into_any_element()
    }
}
