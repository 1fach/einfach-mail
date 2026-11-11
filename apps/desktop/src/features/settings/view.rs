use gpui_kit::component::{
    ColorTokens, StyledExt,
    radio::{Radio, RadioGroup},
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{App, IntoElement, Pixels, Role, Window, div, prelude::*};

use super::{AppearancePreference, AppearanceSettingsNotice};

pub(crate) struct SettingsViewState {
    appearance: AppearancePreference,
    notice: Option<AppearanceSettingsNotice>,
    colors: ColorTokens,
    radius: Pixels,
}

impl SettingsViewState {
    pub(crate) fn new(
        appearance: AppearancePreference,
        notice: Option<AppearanceSettingsNotice>,
        colors: ColorTokens,
        radius: Pixels,
    ) -> Self {
        Self {
            appearance,
            notice,
            colors,
            radius,
        }
    }
}

pub(crate) fn render(
    state: SettingsViewState,
    on_change: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id("settings-scroll-viewport")
        .size_full()
        .overflow_y_scrollbar()
        .bg(state.colors.background)
        .child(render_settings_content(state, on_change))
}

fn render_settings_content(
    state: SettingsViewState,
    on_change: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let appearance =
        render_appearance_card(state.appearance, state.colors, state.radius, on_change);
    settings_content_frame()
        .child(render_heading(state.colors))
        .child(appearance)
        .when_some(state.notice, |content, notice| {
            content.child(render_notice(notice, state.colors))
        })
}

fn settings_content_frame() -> gpui_kit::Div {
    div()
        .w_full()
        .max_w_64()
        .mx_auto()
        .p_8()
        .flex()
        .flex_col()
        .gap_6()
}

fn render_heading(colors: ColorTokens) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_2xl().font_semibold().child("Settings"))
        .child(
            div()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("Choose how Einfach Mail displays its workspace."),
        )
}

fn render_appearance_card(
    appearance: AppearancePreference,
    colors: ColorTokens,
    radius: Pixels,
    on_change: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let options = render_appearance_options(appearance, on_change);
    render_appearance_group(colors, radius, options)
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

fn render_appearance_group(
    colors: ColorTokens,
    radius: Pixels,
    options: impl IntoElement,
) -> impl IntoElement {
    appearance_group_frame(colors, radius)
        .child(render_appearance_description(colors))
        .child(options)
}

fn appearance_group_frame(
    colors: ColorTokens,
    radius: Pixels,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id("settings-appearance-group")
        .w_full()
        .role(Role::Group)
        .aria_label("Appearance")
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_4()
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .rounded(radius)
        .p_4()
}

fn render_appearance_description(colors: ColorTokens) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().font_semibold().child("Appearance"))
        .child(
            div()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("Follow the desktop theme or choose light or dark."),
        )
}

fn render_notice(notice: AppearanceSettingsNotice, colors: ColorTokens) -> impl IntoElement {
    let message = notice.message();
    let notice = div()
        .id("settings-appearance-notice")
        .debug_selector(|| "settings-appearance-notice".to_owned())
        .role(Role::Status)
        .text_sm()
        .text_color(colors.destructive);
    #[cfg(test)]
    let notice = notice.test_support();
    notice.child(message)
}
