//! The Fluent palette and the status tints derived from it.
//!
//! `theme/einfach-fluent.json` is the only source of raw colors in the desktop crate. Everything
//! else reads them through `cx.theme()`.

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeSet};
use gpui_kit::{App, Hsla};

const FLUENT_THEME_SET: &str = include_str!("theme/einfach-fluent.json");
const STATUS_TINT_OPACITY: f32 = 0.12;

/// Installs the Fluent light and dark themes as the app's themes and applies the active mode's.
///
/// Call once at startup, after GPUI Kit initializes and before any window opens. Later
/// appearance changes only need `Theme::change`, which reloads the installed themes.
pub(crate) fn install(cx: &mut App) {
    let theme_set: ThemeSet = serde_json::from_str(FLUENT_THEME_SET)
        .expect("the bundled Fluent theme set is valid JSON for GPUI Kit");
    let mut themes = theme_set.themes.into_iter().map(Rc::new);
    let light = themes
        .next()
        .expect("the Fluent theme set starts with the light theme");
    let dark = themes
        .next()
        .expect("the Fluent theme set ends with the dark theme");
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
    });
    let mode = cx.theme().mode;
    Theme::change(mode, None, cx);
}

/// The meaning a status message carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the info bar is the first consumer")
)]
pub(crate) enum StatusTone {
    Informational,
    Success,
    Warning,
    Error,
}

/// Colors for one status message.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the info bar is the first consumer")
)]
pub(crate) struct StatusColors {
    /// A light tint of the tone, drawn behind the message.
    pub(crate) background: Hsla,
    /// The full-strength tone, for the status icon.
    pub(crate) icon: Hsla,
}

/// Returns the colors for `tone` from the active theme.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the info bar is the first consumer")
)]
pub(crate) fn status_colors(tone: StatusTone, cx: &App) -> StatusColors {
    let theme = cx.theme();
    let icon = match tone {
        StatusTone::Informational => theme.info,
        StatusTone::Success => theme.success,
        StatusTone::Warning => theme.warning,
        StatusTone::Error => theme.danger,
    };
    StatusColors {
        background: icon.opacity(STATUS_TINT_OPACITY),
        icon,
    }
}

#[cfg(test)]
mod tests;
