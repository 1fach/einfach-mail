//! A tinted message bar that reports the state of an operation or a setting.

use gpui_kit::component::{ActiveTheme as _, Icon, h_flex};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{App, IntoElement, RenderOnce, Role, SharedString, Window, div, prelude::*};

use super::icon::AppIcon;
use super::theme::{StatusTone, status_colors};

/// A message with a status icon on a tinted surface.
///
/// `InfoBar::status` keeps the polite `Role::Status` announcement and `InfoBar::alert` keeps the
/// assertive `Role::Alert`. Neither can be dismissed, because no notice in the app can be.
#[derive(IntoElement)]
pub(crate) struct InfoBar {
    id: &'static str,
    role: Role,
    tone: StatusTone,
    message: SharedString,
    accessible_label: Option<SharedString>,
}

impl InfoBar {
    /// A polite, informational message.
    pub(crate) fn status(id: &'static str, message: impl Into<SharedString>) -> Self {
        Self::new(id, Role::Status, StatusTone::Informational, message)
    }

    /// An assertive error message.
    pub(crate) fn alert(id: &'static str, message: impl Into<SharedString>) -> Self {
        Self::new(id, Role::Alert, StatusTone::Error, message)
    }

    fn new(
        id: &'static str,
        role: Role,
        tone: StatusTone,
        message: impl Into<SharedString>,
    ) -> Self {
        Self {
            id,
            role,
            tone,
            message: message.into(),
            accessible_label: None,
        }
    }

    /// Replaces the default tone of the constructor.
    pub(crate) fn tone(mut self, tone: StatusTone) -> Self {
        self.tone = tone;
        self
    }

    /// Names the bar with text other than its message.
    pub(crate) fn accessible_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessible_label = Some(label.into());
        self
    }
}

fn tone_icon(tone: StatusTone) -> AppIcon {
    match tone {
        StatusTone::Informational => AppIcon::Info,
        StatusTone::Success => AppIcon::Success,
        StatusTone::Warning => AppIcon::Warning,
        StatusTone::Error => AppIcon::Error,
    }
}

impl RenderOnce for InfoBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = status_colors(self.tone, cx);
        let theme = cx.theme();
        let id = self.id;
        let bar = h_flex()
            .id(id)
            .debug_selector(move || id.to_owned())
            .role(self.role)
            .when_some(self.accessible_label, |bar, label| bar.aria_label(label))
            .w_full()
            .items_start()
            .gap_3()
            .px_4()
            .py_3()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(colors.background)
            .text_sm()
            .text_color(theme.foreground);
        #[cfg(test)]
        let bar = bar.test_support();
        bar.child(
            Icon::new(tone_icon(self.tone))
                .size_4()
                .mt_0p5()
                .flex_shrink_0()
                .text_color(colors.icon),
        )
        .child(div().flex_1().min_w_0().child(self.message))
    }
}
