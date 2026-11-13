use std::fs;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    App, Bounds, Entity, Hsla, Modifiers, Pixels, Role, TestAppContext, VisualTestContext, px, size,
};
use tempfile::tempdir;

use crate::components::{StatusTone, install_theme, status_colors};
use crate::features::{AppearancePreference, AppearanceSettingsNotice, AppearanceSettingsStore};

use super::*;

fn init_fluent_app(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        install_theme(cx);
    });
}

#[gpui_kit::test]
fn settings_navigation_applies_and_persists_appearance_preferences(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let directory = tempdir().expect("temporary settings directory is created");
    let settings_path = directory.path().join("preferences.json");
    let store = AppearanceSettingsStore::at_path(settings_path.clone());
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |window, cx| {
        Workspace::new_with_appearance_store_for_test(
            Ok(AccountServices::new(workflow, factory)),
            Ok(store),
            window,
            cx,
        )
    });

    draw_account_view(context);
    click(context, "nav-settings");

    assert!(
        context.cx.update(|cx| {
            view.read(cx).test_selected_section() == WorkspaceTestSection::Settings
        })
    );
    assert_appearance_radio_is_selected(context, "System");
    assert!(!settings_path.exists());

    for (label, preference, mode) in [
        ("Dark", AppearancePreference::Dark, ThemeMode::Dark),
        ("Light", AppearancePreference::Light, ThemeMode::Light),
    ] {
        click_radio(context, label);
        assert_appearance_radio_is_selected(context, label);
        assert_eq!(
            context.cx.update(|cx| Theme::global(cx).mode),
            mode,
            "the selected appearance updates the active GPUI theme"
        );
        assert_eq!(
            AppearanceSettingsStore::at_path(settings_path.clone())
                .load()
                .expect("saved appearance can be reloaded"),
            preference
        );
    }

    click_radio(context, "System");
    assert_appearance_radio_is_selected(context, "System");
    assert_eq!(
        context.cx.update(|cx| Theme::global(cx).mode),
        context
            .update(|window, _| { AppearancePreference::System.theme_mode(window.appearance()) }),
        "System follows the current window appearance"
    );
    assert_eq!(
        AppearanceSettingsStore::at_path(settings_path)
            .load()
            .expect("System preference can be reloaded"),
        AppearancePreference::System
    );
}

#[gpui_kit::test]
fn saved_appearance_is_applied_when_the_window_opens(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let directory = tempdir().expect("temporary settings directory is created");
    let settings_path = directory.path().join("preferences.json");
    AppearanceSettingsStore::at_path(settings_path.clone())
        .save(AppearancePreference::Dark)
        .expect("saved appearance fixture is written");
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |window, cx| {
        Workspace::new_with_appearance_store_for_test(
            Ok(AccountServices::new(workflow, factory)),
            Ok(AppearanceSettingsStore::at_path(settings_path)),
            window,
            cx,
        )
    });

    draw_account_view(context);

    assert_eq!(
        context.cx.update(|cx| Theme::global(cx).mode),
        ThemeMode::Dark
    );
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).test_appearance_preference()),
        AppearancePreference::Dark
    );
    click(context, "nav-settings");
    assert_appearance_radio_is_selected(context, "Dark");
}

#[gpui_kit::test]
fn invalid_appearance_file_is_preserved_and_falls_back_to_system(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let directory = tempdir().expect("temporary settings directory is created");
    let settings_path = directory.path().join("preferences.json");
    fs::write(&settings_path, b"invalid settings").expect("invalid settings fixture is written");
    let store_path = settings_path.clone();
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |window, cx| {
        Workspace::new_with_appearance_store_for_test(
            Ok(AccountServices::new(workflow, factory)),
            Ok(AppearanceSettingsStore::at_path(store_path)),
            window,
            cx,
        )
    });

    draw_account_view(context);

    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).test_appearance_preference()),
        AppearancePreference::System
    );
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).test_appearance_settings_notice()),
        Some(AppearanceSettingsNotice::LoadFailed)
    );
    assert_eq!(
        fs::read(settings_path).expect("invalid settings remain available for recovery"),
        b"invalid settings"
    );
    click(context, "nav-settings");
    assert_appearance_radio_is_selected(context, "System");
}

#[gpui_kit::test]
fn appearance_save_failure_keeps_the_session_theme_and_reports_the_failure(
    cx: &mut TestAppContext,
) {
    init_fluent_app(cx);
    let directory = tempdir().expect("temporary settings directory is created");
    let parent_file = directory.path().join("not-a-directory");
    fs::write(&parent_file, b"fixture").expect("parent file fixture is written");
    let settings_path = parent_file.join("preferences.json");
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |window, cx| {
        Workspace::new_with_appearance_store_for_test(
            Ok(AccountServices::new(workflow, factory)),
            Ok(AppearanceSettingsStore::at_path(settings_path)),
            window,
            cx,
        )
    });

    draw_account_view(context);
    click(context, "nav-settings");
    click_radio(context, "Dark");

    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).test_appearance_preference()),
        AppearancePreference::Dark
    );
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).test_appearance_settings_notice()),
        Some(AppearanceSettingsNotice::SaveFailed)
    );
    assert_eq!(
        context.cx.update(|cx| Theme::global(cx).mode),
        ThemeMode::Dark
    );
    context
        .cx
        .update(|cx| assert_settings_text_contrast(Theme::global(cx)));
    assert!(context.debug_bounds("settings-appearance-notice").is_some());
}

#[gpui_kit::test]
fn settings_appearance_keeps_text_contrast_focus_and_layout_across_themes(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let directory = tempdir().expect("temporary settings directory is created");
    let store = AppearanceSettingsStore::at_path(directory.path().join("preferences.json"));
    let (view, context) = settings_view(cx, store);

    draw_account_view(context);
    click(context, "nav-settings");
    focus_settings_radio(&view, context);

    let light_layouts = settings_layouts_for_theme(context, "Light", ThemeMode::Light);
    let dark_layouts = settings_layouts_for_theme(context, "Dark", ThemeMode::Dark);
    assert_layouts_stable_across_themes(light_layouts, dark_layouts);
}

const SETTINGS_VIEWPORT_WIDTHS: [f32; 2] = [1280., 640.];

#[derive(Debug, PartialEq)]
struct SettingsLayout {
    window: gpui_kit::Bounds<gpui_kit::Pixels>,
    appearance_group: gpui_kit::Bounds<gpui_kit::Pixels>,
    radios: Vec<gpui_kit::Bounds<gpui_kit::Pixels>>,
}

fn settings_view(
    cx: &mut TestAppContext,
    store: AppearanceSettingsStore,
) -> (Entity<Workspace<TestProvider>>, &mut VisualTestContext) {
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    cx.add_window_view(move |window, cx| {
        Workspace::new_with_appearance_store_for_test(
            Ok(AccountServices::new(workflow, factory)),
            Ok(store),
            window,
            cx,
        )
    })
}

fn focus_settings_radio(view: &Entity<Workspace<TestProvider>>, context: &mut VisualTestContext) {
    let settings_focus = context.cx.update(|cx| {
        view.read(cx)
            .test_navigation_focus(WorkspaceTestSection::Settings)
    });
    context.update(|window, cx| {
        settings_focus.focus(window, cx);
        assert!(settings_focus.is_focused(window));
        window.focus_next(cx);
    });
    draw_account_view(context);
    assert_settings_radio_is_focused(context);
}

fn settings_layouts_for_theme(
    context: &mut VisualTestContext,
    label: &str,
    expected_mode: ThemeMode,
) -> [SettingsLayout; 2] {
    click_radio(context, label);
    assert_settings_theme_accessibility(context, expected_mode);

    SETTINGS_VIEWPORT_WIDTHS.map(|width| {
        context.simulate_resize(size(px(width), px(720.)));
        context.run_until_parked();
        draw_account_view(context);
        assert_settings_layout_fits(context);
        settings_layout(context)
    })
}

fn assert_settings_theme_accessibility(context: &mut VisualTestContext, expected_mode: ThemeMode) {
    assert_eq!(
        context.cx.update(|cx| Theme::global(cx).mode),
        expected_mode
    );
    context
        .cx
        .update(|cx| assert_settings_text_contrast(Theme::global(cx)));
    assert!(context.cx.update(|cx| Theme::global(cx).focus_ring));
    assert_settings_radio_is_focused(context);
}

fn assert_layouts_stable_across_themes(
    light_layouts: [SettingsLayout; 2],
    dark_layouts: [SettingsLayout; 2],
) {
    for (width, (light, dark)) in SETTINGS_VIEWPORT_WIDTHS
        .into_iter()
        .zip(light_layouts.into_iter().zip(dark_layouts))
    {
        assert_eq!(
            light, dark,
            "changing themes does not move Settings controls at {width}px"
        );
    }
}

fn click_radio(context: &mut VisualTestContext, label: &str) {
    let bounds = context.update(|window, _| window.find_by_label(label).bounds());
    context.simulate_click(bounds.center(), Modifiers::default());
    context.run_until_parked();
    draw_account_view(context);
}

fn assert_appearance_radio_is_selected(context: &mut VisualTestContext, label: &str) {
    let radios = context.update(|window, _| window.find_all_by_role(Role::RadioButton));
    assert_eq!(
        radios.len(),
        3,
        "Settings exposes only the three supported choices"
    );
    let selected: Vec<_> = radios
        .iter()
        .filter(|radio| radio.checked() == Some(true))
        .collect();
    assert_eq!(selected.len(), 1, "one appearance choice is selected");
    assert_eq!(selected[0].label(), Some(label));
    assert_eq!(
        radios
            .iter()
            .filter_map(|radio| radio.label())
            .collect::<Vec<_>>(),
        ["System", "Light", "Dark"]
    );
}

fn assert_settings_radio_is_focused(context: &mut VisualTestContext) {
    let focused_count = context.update(|window, _| {
        window
            .find_all_by_role(Role::RadioButton)
            .iter()
            .filter(|radio| radio.focused() == Some(true))
            .count()
    });
    assert_eq!(focused_count, 1, "Tab reaches one focused appearance radio");
}

fn assert_settings_layout_fits(context: &mut VisualTestContext) {
    let layout = settings_layout(context);
    let group = layout.appearance_group;
    assert!(group.size.width > px(0.) && group.size.height > px(0.));
    assert!(group.left() >= layout.window.left());
    assert!(group.right() <= layout.window.right());
    assert!(group.top() >= layout.window.top());
    assert!(group.bottom() <= layout.window.bottom());
    assert_eq!(layout.radios.len(), 3);
    for bounds in layout.radios {
        assert!(bounds.left() > group.left());
        assert!(bounds.right() < group.right());
        assert!(bounds.top() > group.top());
        assert!(bounds.bottom() < group.bottom());
    }
}

fn settings_layout(context: &mut VisualTestContext) -> SettingsLayout {
    let window = context.update(|window, _| window.bounds());
    let appearance_group = context
        .debug_bounds("settings-appearance-group")
        .expect("the appearance group lays out");
    let radios = context.update(|window, _| {
        window
            .find_all_by_role(Role::RadioButton)
            .into_iter()
            .map(|radio| radio.bounds())
            .collect()
    });
    SettingsLayout {
        window,
        appearance_group,
        radios,
    }
}

fn assert_settings_text_contrast(theme: &Theme) {
    for (label, foreground, background) in [
        ("primary text/page", theme.foreground, theme.background),
        (
            "primary text/card",
            theme.group_box_foreground,
            theme.group_box,
        ),
        ("muted text/page", theme.muted_foreground, theme.background),
        ("muted text/card", theme.muted_foreground, theme.group_box),
    ] {
        assert_contrast_ratio(foreground, background, 4.5, label);
    }
    assert_contrast_ratio(theme.danger, theme.background, 3.0, "error marker");
    assert_contrast_ratio(
        theme.ring.alpha(0.5),
        theme.group_box,
        3.0,
        "focused control ring",
    );
}

fn assert_contrast_ratio(foreground: Hsla, background: Hsla, minimum: f32, label: &str) {
    let ratio = contrast_ratio(foreground, background);
    assert!(
        ratio >= minimum,
        "{label} has {ratio:.2}:1 contrast, below the {minimum}:1 minimum"
    );
}

fn contrast_ratio(foreground: Hsla, background: Hsla) -> f32 {
    let background_rgb = background.to_rgb();
    let foreground_rgb = background_rgb.blend(foreground.to_rgb());
    let foreground_luminance =
        relative_luminance(foreground_rgb.r, foreground_rgb.g, foreground_rgb.b);
    let background_luminance =
        relative_luminance(background_rgb.r, background_rgb.g, background_rgb.b);
    let lighter = foreground_luminance.max(background_luminance);
    let darker = foreground_luminance.min(background_luminance);
    (lighter + 0.05) / (darker + 0.05)
}

fn relative_luminance(red: f32, green: f32, blue: f32) -> f32 {
    let linearize = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linearize(red) + 0.7152 * linearize(green) + 0.0722 * linearize(blue)
}

#[gpui_kit::test]
fn fluent_palette_meets_contrast_minimums_in_light_and_dark(cx: &mut TestAppContext) {
    init_fluent_app(cx);

    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        cx.update(|cx| Theme::change(mode, None, cx));
        cx.read(|cx| assert_fluent_contrast(Theme::global(cx), mode));
    }
}

fn assert_fluent_contrast(theme: &Theme, mode: ThemeMode) {
    let selected_row = theme.background.blend(theme.list_active);
    let page = ("page", theme.background);
    let navigation = ("navigation", theme.sidebar);
    let card = ("card", theme.group_box);
    let popover = ("popover", theme.popover);
    let hover = ("row hover", theme.list_hover);
    let selected = ("selected row", selected_row);

    for (role, foreground, minimum, surfaces) in [
        (
            "primary text",
            theme.foreground,
            4.5,
            vec![
                page,
                navigation,
                card,
                popover,
                hover,
                selected,
                ("selected navigation item", theme.secondary_active),
                ("subtle hover", theme.accent),
            ],
        ),
        (
            "secondary text",
            theme.muted_foreground,
            4.5,
            vec![
                page,
                navigation,
                card,
                popover,
                hover,
                selected,
                ("group header", theme.list_head),
            ],
        ),
        (
            "accent text",
            theme.primary,
            4.5,
            vec![page, navigation, card, selected],
        ),
        (
            "text on accent",
            theme.primary_foreground,
            4.5,
            vec![
                ("rest", theme.primary),
                ("hover", theme.primary_hover),
                ("pressed", theme.primary_active),
            ],
        ),
        (
            "text on danger",
            theme.danger_foreground,
            4.5,
            vec![("button", theme.danger)],
        ),
        (
            "control outline",
            theme.input,
            3.0,
            vec![page, card, popover],
        ),
        (
            "selection pill",
            theme.primary,
            3.0,
            vec![
                navigation,
                ("selected navigation item", theme.secondary_active),
            ],
        ),
    ] {
        for (surface, background) in surfaces {
            assert_contrast_ratio(
                foreground,
                background,
                minimum,
                &format!("{mode:?} {role}/{surface}"),
            );
        }
    }
}

#[gpui_kit::test]
fn fluent_focus_ring_and_status_colors_stay_visible(cx: &mut TestAppContext) {
    init_fluent_app(cx);

    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        cx.update(|cx| Theme::change(mode, None, cx));
        cx.read(|cx| {
            let theme = Theme::global(cx);
            for (surface, background) in [
                ("page", theme.background),
                ("navigation", theme.sidebar),
                ("card", theme.group_box),
                ("popover", theme.popover),
            ] {
                assert_contrast_ratio(
                    theme.ring,
                    background,
                    3.0,
                    &format!("{mode:?} focus ring/{surface}"),
                );
                assert_contrast_ratio(
                    theme.ring.alpha(0.5),
                    background,
                    3.0,
                    &format!("{mode:?} half-opacity focus halo/{surface}"),
                );
            }
            assert_status_contrast(theme, cx, mode);
        });
    }
}

fn assert_status_contrast(theme: &Theme, cx: &App, mode: ThemeMode) {
    for tone in [
        StatusTone::Informational,
        StatusTone::Success,
        StatusTone::Warning,
        StatusTone::Error,
    ] {
        let colors = status_colors(tone, cx);
        for (surface, background) in [("page", theme.background), ("card", theme.group_box)] {
            let tinted = background.blend(colors.background);
            assert_contrast_ratio(
                colors.icon,
                background,
                3.0,
                &format!("{mode:?} {tone:?} icon/{surface}"),
            );
            assert_contrast_ratio(
                colors.icon,
                tinted,
                3.0,
                &format!("{mode:?} {tone:?} icon/tinted {surface}"),
            );
            assert_contrast_ratio(
                theme.foreground,
                tinted,
                4.5,
                &format!("{mode:?} {tone:?} text/tinted {surface}"),
            );
        }
    }
}

const PAGE_CONTENT_WIDTH: Pixels = px(640.);
const PAGE_PADDING: Pixels = px(32.);

fn navigation_bounds(context: &mut VisualTestContext) -> Bounds<Pixels> {
    context.update(|window, _| window.find_by_label("Main navigation").bounds())
}

/// Checks that the named surfaces share one left and right edge, stack downward without
/// overlapping, and stay inside the page column, which is centered beside the navigation pane.
fn assert_page_column(context: &mut VisualTestContext, selectors: &[&'static str], note: &str) {
    let window = context.update(|window, _| window.bounds());
    let navigation = navigation_bounds(context);
    let surfaces: Vec<Bounds<Pixels>> = selectors
        .iter()
        .map(|selector| {
            context
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{note}: `{selector}` lays out"))
        })
        .collect();
    let first = surfaces[0];
    let content_left = navigation.right();
    assert!(
        first.top() >= window.top() && first.top() < window.top() + px(300.),
        "{note}: the page starts at the top, not the middle"
    );
    assert!(
        first.size.width <= PAGE_CONTENT_WIDTH,
        "{note}: the column is at most 40rem"
    );
    assert!(
        first.left() >= content_left + PAGE_PADDING
            && first.right() <= window.right() - PAGE_PADDING,
        "{note}: the column keeps the page padding"
    );
    assert!(
        (first.left() - content_left - (window.right() - first.right())).abs() <= px(1.),
        "{note}: the column is centered beside the navigation pane"
    );
    for pair in surfaces.windows(2) {
        assert_eq!(pair[0].left(), pair[1].left(), "{note}: one left spine");
        assert_eq!(pair[0].right(), pair[1].right(), "{note}: one right spine");
        assert!(
            pair[1].top() >= pair[0].bottom(),
            "{note}: surfaces stack without overlap"
        );
    }
}

#[gpui_kit::test]
fn settings_card_and_info_bar_share_one_column_in_both_themes_and_widths(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let directory = tempdir().expect("temporary settings directory is created");
    let parent_file = directory.path().join("not-a-directory");
    fs::write(&parent_file, b"fixture").expect("parent file fixture is written");
    let store = AppearanceSettingsStore::at_path(parent_file.join("preferences.json"));
    let (_view, context) = settings_view(cx, store);

    draw_account_view(context);
    click(context, "nav-settings");

    let mut card_heights = Vec::new();
    for (label, mode) in [("Light", ThemeMode::Light), ("Dark", ThemeMode::Dark)] {
        click_radio(context, label);
        assert_eq!(context.cx.update(|cx| Theme::global(cx).mode), mode);
        context.cx.update(|cx| {
            let theme = Theme::global(cx);
            assert_settings_text_contrast(theme);
            for tone in [StatusTone::Warning, StatusTone::Error] {
                let status = status_colors(tone, cx);
                let tinted = theme.background.blend(status.background);
                assert_contrast_ratio(theme.foreground, tinted, 4.5, "notice text");
                assert_contrast_ratio(status.icon, tinted, 3.0, "notice icon");
            }
        });
        for width in SETTINGS_VIEWPORT_WIDTHS {
            context.simulate_resize(size(px(width), px(900.)));
            context.run_until_parked();
            draw_account_view(context);
            assert_page_column(
                context,
                &["settings-appearance-group", "settings-appearance-notice"],
                &format!("{label} at {width}px"),
            );
            card_heights.push(
                context
                    .debug_bounds("settings-appearance-group")
                    .expect("the appearance card lays out")
                    .size
                    .height,
            );
        }
    }

    for themed_heights in card_heights.chunks(2) {
        assert!(
            themed_heights[1] > themed_heights[0],
            "the trailing radios wrap below the card text at 640px"
        );
    }
}

fn accounts_view(
    cx: &mut TestAppContext,
    services: AccountServices<TestProvider>,
) -> (Entity<Workspace<TestProvider>>, &mut VisualTestContext) {
    let (view, context) = cx.add_window_view(move |_, cx| Workspace::new(Ok(services), cx));
    context.run_until_parked();
    draw_account_view(context);
    (view, context)
}

fn assert_accounts_column_in_both_themes_and_widths(
    context: &mut VisualTestContext,
    selectors: &[&'static str],
) {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        context.cx.update(|cx| Theme::change(mode, None, cx));
        for width in SETTINGS_VIEWPORT_WIDTHS {
            context.simulate_resize(size(px(width), px(2000.)));
            context.run_until_parked();
            draw_account_view(context);
            assert_page_column(
                context,
                selectors,
                &format!("Accounts {mode:?} at {width}px"),
            );
        }
    }
}

#[gpui_kit::test]
fn accounts_provider_cards_start_at_the_top_of_one_column(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let services = AccountServices::new(workflow, unused_account_session_factory());
    let (_view, context) = accounts_view(cx, services);

    assert_accounts_column_in_both_themes_and_widths(
        context,
        &[
            "microsoft365-provider",
            "account-requested-access",
            "exchange-ews-provider",
        ],
    );
}

#[gpui_kit::test]
fn accounts_setup_notice_sits_between_the_provider_cards(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let services = AccountServices::new(workflow, unused_account_session_factory())
        .with_microsoft365_setup_issue(crate::features::SetupIssue::MissingClientId);
    let (_view, context) = accounts_view(cx, services);

    assert_accounts_column_in_both_themes_and_widths(
        context,
        &[
            "microsoft365-provider",
            "microsoft365-setup-status",
            "exchange-ews-provider",
        ],
    );
}

#[gpui_kit::test]
fn connected_account_identity_card_keeps_sign_out_inside_the_column(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let services = AccountServices::new(workflow, unused_account_session_factory());
    let (view, context) = accounts_view(cx, services);
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.test_set_connected_account(connected_account(mail_only_profile()), cx);
        });
    });
    draw_account_view(context);

    assert_accounts_column_in_both_themes_and_widths(context, &["connected-account"]);
    for width in SETTINGS_VIEWPORT_WIDTHS {
        context.simulate_resize(size(px(width), px(2000.)));
        context.run_until_parked();
        draw_account_view(context);
        let card = context
            .debug_bounds("connected-account")
            .expect("the identity card lays out");
        let sign_out = context
            .debug_bounds("sign-out")
            .expect("Sign out is a trailing control of the identity card");
        assert!(
            sign_out.left() >= card.left() && sign_out.right() <= card.right(),
            "Sign out stays inside the identity card at {width}px"
        );
    }
}

#[gpui_kit::test]
fn opening_progress_stays_on_the_account_page(cx: &mut TestAppContext) {
    init_fluent_app(cx);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let services = AccountServices::new(workflow, unused_account_session_factory());
    let (view, context) = accounts_view(cx, services);
    context.cx.update(|cx| {
        view.update(cx, |view, cx| view.test_set_opening_mail_content(cx));
    });
    draw_account_view(context);

    let window = context.update(|window, _| window.bounds());
    let progress = context
        .debug_bounds("account-operation-progress")
        .expect("the opening progress shows on the account page");
    assert!(progress.bottom() <= window.bottom());
    assert!(progress.left() > navigation_bounds(context).right());
}
