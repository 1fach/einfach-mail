use std::ffi::OsStr;

use super::{
    NavigationSurfaceAppearance, SetupIssue, configured_callback_port,
    navigation_surface_appearance_from_environment, window_options,
};
use crate::components::NAVIGATION_PANE_WIDTH;
use crate::features::MAIL_MINIMUM_WIDTH;
use einfach_mail_microsoft_graph::CallbackPort;
use gpui_kit::{WindowBackgroundAppearance, px};

#[test]
fn desktop_auth_requires_a_registered_fixed_loopback_port() {
    assert_eq!(
        configured_callback_port(None),
        Err(SetupIssue::MissingCallbackPort)
    );
    assert_eq!(
        configured_callback_port(Some("0")),
        Err(SetupIssue::InvalidCallbackPort)
    );
    assert_eq!(
        configured_callback_port(Some("invalid")),
        Err(SetupIssue::InvalidCallbackPort)
    );
    assert!(matches!(
        configured_callback_port(Some("43123")),
        Ok(CallbackPort::Fixed(port)) if port.get() == 43123
    ));
}

#[test]
fn navigation_translucency_requires_explicit_wayland_opt_in() {
    assert_eq!(
        navigation_surface_appearance_from_environment(None, Some(OsStr::new("wayland-0"))),
        NavigationSurfaceAppearance::Opaque
    );
    assert_eq!(
        navigation_surface_appearance_from_environment(
            Some(OsStr::new("1")),
            Some(OsStr::new("wayland-0"))
        ),
        NavigationSurfaceAppearance::Translucent
    );
    assert_eq!(
        navigation_surface_appearance_from_environment(Some(OsStr::new("1")), Some(OsStr::new(""))),
        NavigationSurfaceAppearance::Opaque
    );
    assert_eq!(
        navigation_surface_appearance_from_environment(Some(OsStr::new("1")), None),
        NavigationSurfaceAppearance::Opaque
    );
}

#[test]
fn window_background_matches_navigation_surface_appearance() {
    assert_eq!(
        window_options(NavigationSurfaceAppearance::Opaque, px(16.)).window_background,
        WindowBackgroundAppearance::Opaque
    );
    assert_eq!(
        window_options(NavigationSurfaceAppearance::Translucent, px(16.)).window_background,
        WindowBackgroundAppearance::Transparent
    );
}

#[test]
fn window_minimum_keeps_the_navigation_pane_beside_the_narrowest_mail_layout() {
    let rem_size = px(16.);
    let minimum = window_options(NavigationSurfaceAppearance::Opaque, rem_size)
        .window_min_size
        .expect("the main window sets a minimum size");

    assert_eq!(
        minimum.width,
        NAVIGATION_PANE_WIDTH.to_pixels(rem_size) + MAIL_MINIMUM_WIDTH.to_pixels(rem_size)
    );
    assert!(minimum.height > px(0.));
}
