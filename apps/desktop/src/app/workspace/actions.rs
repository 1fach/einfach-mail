use std::sync::Arc;

use einfach_mail_application::{AccountCacheOpenError, AccountPermissionRequest, AccountSession};
use einfach_mail_domain::AccountProfile;
use gpui_kit::{App, AppContext, AsyncApp, Context, Task, Window};

use crate::features::{
    AccountViewEvent, CalendarView, CalendarViewEvent, MailFolderNavigation, MailView,
    MailViewEvent, SignOutSessionDisposition,
};

use super::Workspace;
use super::state::{OpeningContent, WorkspaceContent, WorkspaceSection};

impl<P: einfach_mail_application::AccountProvider + 'static> Workspace<P> {
    pub(super) fn navigate_to(&mut self, section: WorkspaceSection, cx: &mut Context<Self>) {
        if self.content.selected_section() == section
            || self.navigation_disabled_reason(section, cx).is_some()
        {
            return;
        }
        self.close_content_view(cx);
        self.content = WorkspaceContent::Accounts;
        match section {
            WorkspaceSection::Accounts => cx.notify(),
            WorkspaceSection::Settings => {
                self.content = WorkspaceContent::Settings;
                cx.notify();
            }
            WorkspaceSection::Mail => self.open_mail(cx),
            WorkspaceSection::Calendar => self.open_calendar(cx),
        }
    }

    pub(super) fn navigation_disabled_reason(
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
        self.section_disabled_reason(section, cx)
    }

    fn section_disabled_reason(&self, section: WorkspaceSection, cx: &App) -> Option<&'static str> {
        if matches!(
            section,
            WorkspaceSection::Accounts | WorkspaceSection::Settings
        ) {
            return None;
        }
        let account_view = self.account_view.read(cx);
        if let Some(reason) = account_view.navigation_disabled_reason() {
            return Some(reason);
        }
        if section == WorkspaceSection::Calendar && !account_view.calendar_is_available() {
            return Some("Calendar is unavailable for this account.");
        }
        None
    }

    fn can_navigate_away(&self, cx: &App) -> bool {
        match &self.content {
            WorkspaceContent::Mail { view, .. } => view.read(cx).can_navigate_away(),
            WorkspaceContent::Accounts
            | WorkspaceContent::Settings
            | WorkspaceContent::Opening(_)
            | WorkspaceContent::Calendar { .. } => true,
        }
    }

    fn open_mail(&mut self, cx: &mut Context<Self>) {
        if self.mail_permission_requester.is_none() {
            return;
        }
        self.open_account_content(OpeningContent::Mail, cx);
    }

    fn open_calendar(&mut self, cx: &mut Context<Self>) {
        if !self.account_view.read(cx).calendar_is_available() {
            return;
        }
        self.open_account_content(OpeningContent::Calendar, cx);
    }

    fn open_account_content(&mut self, opening: OpeningContent, cx: &mut Context<Self>) {
        if !matches!(&self.content, WorkspaceContent::Accounts) {
            return;
        }
        let Some(profile) = self.account_view.read(cx).connected_profile_for_workspace() else {
            return;
        };
        if self.account_session.is_none() && self.account_session_factory.is_none() {
            return;
        }
        self.begin_content_open(opening, profile, cx);
    }

    fn begin_content_open(
        &mut self,
        opening: OpeningContent,
        profile: AccountProfile,
        cx: &mut Context<Self>,
    ) {
        let retained_session = self.account_session.clone();
        let account_session_factory = self.account_session_factory.clone();
        self.content = WorkspaceContent::Opening(opening);
        self.account_view.update(cx, |account_view, cx| {
            account_view.set_workspace_opening(true, cx);
            account_view.clear_cache_removal_error(cx);
        });
        self.session_open_error = None;
        self.task = Some(Self::open_content_task(
            opening,
            profile,
            retained_session,
            account_session_factory,
            cx,
        ));
        cx.notify();
    }

    fn open_content_task(
        opening: OpeningContent,
        profile: AccountProfile,
        retained_session: Option<Arc<dyn AccountSession>>,
        account_session_factory: Option<Arc<dyn einfach_mail_application::AccountSessionFactory>>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let session =
                Self::open_session(retained_session, account_session_factory, &profile, cx).await;
            let _ = this.update(cx, move |workspace, cx| {
                workspace.finish_open_content(opening, profile, session, cx);
            });
        })
    }

    async fn open_session(
        retained_session: Option<Arc<dyn AccountSession>>,
        account_session_factory: Option<Arc<dyn einfach_mail_application::AccountSessionFactory>>,
        profile: &AccountProfile,
        cx: &mut AsyncApp,
    ) -> Result<Arc<dyn AccountSession>, einfach_mail_application::AccountCacheOpenError> {
        match retained_session {
            Some(session) => Ok(session),
            None => {
                let account_id = profile.id().clone();
                let factory = account_session_factory
                    .expect("a new connected workspace session has a session factory");
                cx.background_executor()
                    .spawn(async move { factory.open_session(account_id).await })
                    .await
            }
        }
    }

    fn finish_open_content(
        &mut self,
        opening: OpeningContent,
        profile: AccountProfile,
        session: Result<Arc<dyn AccountSession>, AccountCacheOpenError>,
        cx: &mut Context<Self>,
    ) {
        if self.content.opening_kind() != Some(opening)
            || !self.account_view.read(cx).is_connected_to(profile.id())
        {
            return;
        }
        self.task = None;
        self.content = WorkspaceContent::Accounts;
        self.account_view.update(cx, |account_view, cx| {
            account_view.set_workspace_opening(false, cx)
        });
        self.install_open_content(opening, profile, session, cx);
        cx.notify();
    }

    fn install_open_content(
        &mut self,
        opening: OpeningContent,
        profile: AccountProfile,
        session: Result<Arc<dyn AccountSession>, AccountCacheOpenError>,
        cx: &mut Context<Self>,
    ) {
        match session {
            Ok(session) => {
                self.content = self.build_open_content(opening, profile, Arc::clone(&session), cx);
                self.account_session = Some(session);
            }
            Err(error) => self.session_open_error = Some(error),
        }
    }

    fn build_open_content(
        &mut self,
        opening: OpeningContent,
        profile: AccountProfile,
        session: Arc<dyn AccountSession>,
        cx: &mut Context<Self>,
    ) -> WorkspaceContent {
        match opening {
            OpeningContent::Mail => self.build_mail_content(profile, session, cx),
            OpeningContent::Calendar => self.build_calendar_content(profile, session, cx),
        }
    }

    fn build_mail_content(
        &mut self,
        profile: AccountProfile,
        session: Arc<dyn AccountSession>,
        cx: &mut Context<Self>,
    ) -> WorkspaceContent {
        let mail_session = session.mail_session();
        let permission_requester = self
            .mail_permission_requester
            .as_ref()
            .expect("mail navigation has an account permission requester")
            .clone();
        let mail_view = cx.new(|cx| MailView::new(profile, mail_session, permission_requester, cx));
        let folder_navigation = cx.new(|cx| MailFolderNavigation::new(mail_view.clone(), cx));
        let subscription = cx.subscribe(&mail_view, |workspace, _, event: &MailViewEvent, cx| {
            if matches!(event, MailViewEvent::ReviewMailWritePermission) {
                workspace.review_permission(AccountPermissionRequest::MailReadWrite, cx);
            }
        });
        WorkspaceContent::Mail {
            view: mail_view,
            folder_navigation,
            subscription,
        }
    }

    fn build_calendar_content(
        &mut self,
        profile: AccountProfile,
        session: Arc<dyn AccountSession>,
        cx: &mut Context<Self>,
    ) -> WorkspaceContent {
        let calendar_session = session.calendar_session();
        let calendar_view = cx.new(|cx| CalendarView::new(profile, calendar_session, cx));
        let subscription = cx.subscribe(
            &calendar_view,
            |workspace, _, event: &CalendarViewEvent, cx| {
                if matches!(event, CalendarViewEvent::ReviewCalendarWritePermission) {
                    workspace.review_permission(AccountPermissionRequest::CalendarReadWrite, cx);
                }
            },
        );
        WorkspaceContent::Calendar {
            view: calendar_view,
            subscription,
        }
    }

    fn review_permission(&mut self, request: AccountPermissionRequest, cx: &mut Context<Self>) {
        self.close_content_view(cx);
        self.content = WorkspaceContent::Accounts;
        self.account_view.update(cx, |account_view, cx| {
            account_view.set_workspace_opening(false, cx);
            account_view.prepare_permission_review(request, cx);
        });
        cx.notify();
    }

    fn close_content_view(&mut self, cx: &mut Context<Self>) {
        match std::mem::replace(&mut self.content, WorkspaceContent::Accounts) {
            WorkspaceContent::Mail {
                view,
                folder_navigation,
                subscription,
            } => {
                drop(subscription);
                drop(folder_navigation);
                view.update(cx, |view, _| view.stop_pending_work());
            }
            WorkspaceContent::Calendar { view, subscription } => {
                drop(subscription);
                view.update(cx, |view, _| view.stop_pending_work());
            }
            content => self.content = content,
        }
    }

    pub(super) fn on_account_view_event(
        &mut self,
        event: &AccountViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            AccountViewEvent::RestoreStarted => {
                self.account_session = None;
                self.session_open_error = None;
            }
            AccountViewEvent::SignOutRequested => self.sign_out(cx),
            AccountViewEvent::SignOutFinished(disposition) => {
                if *disposition == SignOutSessionDisposition::Release {
                    self.account_session = None;
                }
                cx.notify();
            }
        }
    }

    fn sign_out(&mut self, cx: &mut Context<Self>) {
        if self.content.opening_kind().is_some() {
            return;
        }
        self.close_content_view(cx);
        self.content = WorkspaceContent::Accounts;
        let session = self.account_session.clone();
        self.session_open_error = None;
        self.account_view.update(cx, |account_view, cx| {
            account_view.set_workspace_opening(false, cx);
            account_view.sign_out(session, cx);
        });
        cx.notify();
    }

    pub(super) fn navigation_focus(&self, section: WorkspaceSection) -> &gpui_kit::FocusHandle {
        match section {
            WorkspaceSection::Mail => &self.mail_navigation_focus,
            WorkspaceSection::Calendar => &self.calendar_navigation_focus,
            WorkspaceSection::Accounts => &self.accounts_navigation_focus,
            WorkspaceSection::Settings => &self.settings_navigation_focus,
        }
    }

    pub(super) fn change_appearance(
        &mut self,
        appearance: super::AppearancePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.appearance_preference == appearance && self.appearance_settings_notice.is_none() {
            return;
        }
        self.appearance_preference = appearance;
        self.appearance_settings_notice = match &self.appearance_settings_store {
            Some(store) if store.save(appearance).is_ok() => None,
            Some(_) | None => Some(super::AppearanceSettingsNotice::SaveFailed),
        };
        self.apply_appearance(window, cx);
        cx.notify();
    }
}
