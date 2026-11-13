use std::collections::BTreeSet;

use gpui_kit::assets::{IconName, IconNamed as _};
use gpui_kit::{AssetSource as _, SharedString};

use super::{AppAssets, AppIcon, AppIcons};

fn is_svg(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| text.contains("<svg"))
}

#[test]
fn every_app_icon_loads_from_the_app_assets() {
    for icon in AppIcon::ALL {
        let path = icon.path();
        let bytes = AppAssets
            .load(&path)
            .expect("the asset source does not fail for an app icon")
            .unwrap_or_else(|| panic!("{icon:?} resolves to {path}, which is not embedded"));
        assert!(is_svg(&bytes), "{icon:?} at {path} is an SVG");
    }
}

#[test]
fn app_assets_keep_the_icons_gpui_components_draw() {
    for icon in [
        IconName::Inbox,
        IconName::User,
        IconName::ChevronDown,
        IconName::Check,
    ] {
        let path = icon.path();
        let bytes = AppAssets
            .load(&path)
            .expect("the asset source does not fail for a default icon")
            .unwrap_or_else(|| panic!("{path} is missing from the app assets"));
        assert!(is_svg(&bytes), "{path} is an SVG");
    }
}

#[test]
fn the_embedded_subset_is_exactly_the_icons_app_icon_uses() {
    let used: BTreeSet<SharedString> = AppIcon::ALL.iter().map(|icon| icon.path()).collect();
    let embedded: BTreeSet<SharedString> = AppIcons
        .list("icons/")
        .expect("the subset lists its icons")
        .into_iter()
        .collect();

    assert_eq!(embedded, used);
}

#[test]
fn the_app_assets_list_each_icon_once() {
    let listed = AppAssets
        .list("icons/")
        .expect("the app assets list their icons");
    let unique: BTreeSet<_> = listed.iter().collect();

    assert_eq!(listed.len(), unique.len());
    assert!(
        AppIcon::ALL
            .iter()
            .all(|icon| unique.contains(&icon.path()))
    );
}
