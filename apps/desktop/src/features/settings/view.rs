use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::{App, IntoElement, Window, prelude::*};

use super::{AppearancePreference, AppearanceSettingsNotice};
use crate::components::{AppIcon, InfoBar, Page, SettingsCard, StatusTone};

pub(crate) struct SettingsViewState {
    appearance: AppearancePreference,
    notice: Option<AppearanceSettingsNotice>,
}

impl SettingsViewState {
    pub(crate) fn new(
        appearance: AppearancePreference,
        notice: Option<AppearanceSettingsNotice>,
    ) -> Self {
        Self { appearance, notice }
    }
}

pub(crate) fn render(
    state: SettingsViewState,
    on_change: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let appearance = SettingsCard::new("settings-appearance-group", "Appearance")
        .icon(AppIcon::Appearance)
        .description("Follow the desktop theme or choose light or dark.")
        .trailing(render_appearance_options(state.appearance, on_change));
    Page::new("settings-scroll-viewport", "Settings")
        .description("Choose how Einfach Mail displays its workspace.")
        .child(appearance)
        .when_some(state.notice, |page, notice| {
            page.child(
                InfoBar::status("settings-appearance-notice", notice.message())
                    .tone(notice_tone(notice)),
            )
        })
}

fn notice_tone(notice: AppearanceSettingsNotice) -> StatusTone {
    match notice {
        AppearanceSettingsNotice::LoadFailed => StatusTone::Warning,
        AppearanceSettingsNotice::SaveFailed => StatusTone::Error,
    }
}

fn render_appearance_options(
    appearance: AppearancePreference,
    on_change: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    RadioGroup::horizontal("appearance-preference")
        .selected_index(Some(appearance.radio_index()))
        .children([
            Radio::new("appearance-system").label("System"),
            Radio::new("appearance-light").label("Light"),
            Radio::new("appearance-dark").label("Dark"),
        ])
        .on_change(on_change)
}
