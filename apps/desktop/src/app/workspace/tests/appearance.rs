use std::fs;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{Modifiers, Role, TestAppContext, VisualTestContext};
use tempfile::tempdir;

use crate::features::{AppearancePreference, AppearanceSettingsNotice, AppearanceSettingsStore};

use super::*;

#[gpui_kit::test]
fn settings_navigation_applies_and_persists_appearance_preferences(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let directory = tempdir().expect("temporary settings directory is created");
    let settings_path = directory.path().join("preferences.json");
    let store = AppearanceSettingsStore::at_path(settings_path.clone());
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |window, cx| {
        AccountView::new_with_appearance_store_for_test(
            Ok(AccountServices::new(workflow, factory)),
            Ok(store),
            window,
            cx,
        )
    });

    draw_account_view(context);
    click(context, "nav-settings");

    assert!(context.cx.update(|cx| {
        view.read(cx).test_selected_section() == AccountViewTestSection::Settings
    }));
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
    cx.update(gpui_kit::init);
    let directory = tempdir().expect("temporary settings directory is created");
    let settings_path = directory.path().join("preferences.json");
    AppearanceSettingsStore::at_path(settings_path.clone())
        .save(AppearancePreference::Dark)
        .expect("saved appearance fixture is written");
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |window, cx| {
        AccountView::new_with_appearance_store_for_test(
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
    cx.update(gpui_kit::init);
    let directory = tempdir().expect("temporary settings directory is created");
    let settings_path = directory.path().join("preferences.json");
    fs::write(&settings_path, b"invalid settings").expect("invalid settings fixture is written");
    let store_path = settings_path.clone();
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |window, cx| {
        AccountView::new_with_appearance_store_for_test(
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
    cx.update(gpui_kit::init);
    let directory = tempdir().expect("temporary settings directory is created");
    let parent_file = directory.path().join("not-a-directory");
    fs::write(&parent_file, b"fixture").expect("parent file fixture is written");
    let settings_path = parent_file.join("preferences.json");
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |window, cx| {
        AccountView::new_with_appearance_store_for_test(
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
    assert!(context.debug_bounds("settings-appearance-notice").is_some());
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
