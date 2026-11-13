//! Opens the UI preview on the view chosen by `EINFACH_MAIL_UI_PREVIEW_DESTINATION`.

use einfach_mail_application::AccountProvider;
use gpui_kit::{Context, Entity};

use crate::app::preview::PreviewDestination;
use crate::features::MailView;

use super::Workspace;
use super::state::WorkspaceSection;

/// What the preview still has to do before it shows the requested view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PreviewStartStep {
    /// Navigate once the restored account allows it.
    Navigate(PreviewDestination),
    /// Open the first message once Mail has loaded its cache.
    OpenFirstMessage,
}

impl PreviewDestination {
    fn section(self) -> WorkspaceSection {
        match self {
            Self::Accounts => WorkspaceSection::Accounts,
            Self::Mail | Self::MailReader => WorkspaceSection::Mail,
            Self::Calendar => WorkspaceSection::Calendar,
            Self::Settings => WorkspaceSection::Settings,
        }
    }
}

impl<P: AccountProvider + 'static> Workspace<P> {
    pub(crate) fn start_at_preview_destination(&mut self, destination: PreviewDestination) {
        self.preview_start = Some(PreviewStartStep::Navigate(destination));
    }

    /// Runs on every account view update, so it waits until the account restore allows the
    /// destination and then navigates exactly once.
    pub(super) fn advance_preview_start(&mut self, cx: &mut Context<Self>) {
        let Some(PreviewStartStep::Navigate(destination)) = self.preview_start else {
            return;
        };
        let section = destination.section();
        if self.navigation_disabled_reason(section, cx).is_some() {
            return;
        }
        self.preview_start = (destination == PreviewDestination::MailReader)
            .then_some(PreviewStartStep::OpenFirstMessage);
        self.navigate_to(section, cx);
    }

    /// Opens the first message as soon as the new Mail view has one to select.
    pub(super) fn watch_mail_for_preview_start(
        &mut self,
        mail_view: &Entity<MailView>,
        cx: &mut Context<Self>,
    ) {
        if self.preview_start != Some(PreviewStartStep::OpenFirstMessage) {
            return;
        }
        self._preview_message_observation =
            Some(cx.observe(mail_view, |workspace, mail_view, cx| {
                if workspace.preview_start != Some(PreviewStartStep::OpenFirstMessage) {
                    return;
                }
                if mail_view.update(cx, |view, cx| view.open_first_message_for_preview(cx)) {
                    workspace.preview_start = None;
                }
            }));
    }
}
