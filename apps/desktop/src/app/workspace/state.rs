use gpui_kit::component::IconName;
use gpui_kit::{Background, WindowBackgroundAppearance};

use crate::features::{CalendarView, MailFolderNavigation, MailView};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum OpeningContent {
    Mail,
    Calendar,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum WorkspaceSection {
    Mail,
    Calendar,
    Accounts,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NavigationSurfaceAppearance {
    Opaque,
    // Preview startup uses opaque windows; normal startup retains the translucent opt-in.
    #[cfg_attr(feature = "ui-preview", allow(dead_code))]
    Translucent,
}

impl NavigationSurfaceAppearance {
    pub(super) fn navigation_background(self, sidebar: Background) -> Background {
        match self {
            Self::Opaque => sidebar,
            Self::Translucent => sidebar.opacity(NAVIGATION_SURFACE_OPACITY),
        }
    }

    pub(crate) fn window_background(self) -> WindowBackgroundAppearance {
        match self {
            Self::Opaque => WindowBackgroundAppearance::Opaque,
            Self::Translucent => WindowBackgroundAppearance::Transparent,
        }
    }
}

pub(super) const NAVIGATION_SURFACE_OPACITY: f32 = 0.88;

impl WorkspaceSection {
    pub(super) fn navigation_identity(self) -> (&'static str, &'static str, IconName) {
        match self {
            Self::Mail => ("nav-mail", "Mail", IconName::Inbox),
            Self::Calendar => ("nav-calendar", "Calendar", IconName::Calendar),
            Self::Accounts => ("nav-accounts", "Accounts", IconName::User),
            Self::Settings => ("nav-settings", "Settings", IconName::Settings),
        }
    }
}

pub(super) enum WorkspaceContent {
    Accounts,
    Settings,
    Opening(OpeningContent),
    Mail {
        view: gpui_kit::Entity<MailView>,
        folder_navigation: gpui_kit::Entity<MailFolderNavigation>,
        subscription: gpui_kit::Subscription,
    },
    Calendar {
        view: gpui_kit::Entity<CalendarView>,
        subscription: gpui_kit::Subscription,
    },
}

impl WorkspaceContent {
    pub(super) fn opening_kind(&self) -> Option<OpeningContent> {
        match self {
            Self::Opening(opening) => Some(*opening),
            Self::Accounts | Self::Settings | Self::Mail { .. } | Self::Calendar { .. } => None,
        }
    }

    pub(super) fn selected_section(&self) -> WorkspaceSection {
        match self {
            Self::Mail { .. } => WorkspaceSection::Mail,
            Self::Calendar { .. } => WorkspaceSection::Calendar,
            Self::Settings => WorkspaceSection::Settings,
            Self::Opening(OpeningContent::Mail) => WorkspaceSection::Mail,
            Self::Opening(OpeningContent::Calendar) => WorkspaceSection::Calendar,
            Self::Accounts => WorkspaceSection::Accounts,
        }
    }
}
