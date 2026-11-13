use super::{PreviewDestination, PreviewStart, PreviewWindowSize};
use crate::features::AppearancePreference;

#[test]
fn omitted_choices_keep_the_default_preview() {
    assert_eq!(
        PreviewStart::parse(None, None, None).expect("defaults are valid"),
        PreviewStart {
            destination: PreviewDestination::Accounts,
            appearance: None,
            window_size: None,
        }
    );
}

#[test]
fn every_documented_choice_parses() {
    for (value, destination) in [
        ("accounts", PreviewDestination::Accounts),
        ("mail", PreviewDestination::Mail),
        ("mail-reader", PreviewDestination::MailReader),
        ("calendar", PreviewDestination::Calendar),
        ("settings", PreviewDestination::Settings),
    ] {
        assert_eq!(
            PreviewStart::parse(Some(value), None, None)
                .expect("documented destination is valid")
                .destination,
            destination
        );
    }
    assert_eq!(
        PreviewStart::parse(None, Some("dark"), Some("900x700")).expect("choices are valid"),
        PreviewStart {
            destination: PreviewDestination::Accounts,
            appearance: Some(AppearancePreference::Dark),
            window_size: Some(PreviewWindowSize {
                width: 900,
                height: 700,
            }),
        }
    );
    assert_eq!(
        PreviewStart::parse(None, Some("light"), None)
            .expect("light is valid")
            .appearance,
        Some(AppearancePreference::Light)
    );
}

#[test]
fn unknown_choices_name_the_variable_and_the_accepted_values() {
    for ((destination, appearance), variable) in [
        ((Some("inbox"), None), "EINFACH_MAIL_UI_PREVIEW_DESTINATION"),
        ((Some(""), None), "EINFACH_MAIL_UI_PREVIEW_DESTINATION"),
        ((None, Some("system")), "EINFACH_MAIL_UI_PREVIEW_APPEARANCE"),
        ((None, Some("Dark")), "EINFACH_MAIL_UI_PREVIEW_APPEARANCE"),
    ] {
        let error = PreviewStart::parse(destination, appearance, None)
            .expect_err("unknown preview choice is rejected");
        assert!(
            error.to_string().contains(variable),
            "{error} names {variable}"
        );
    }
}

#[test]
fn window_sizes_need_two_positive_bounded_edges() {
    for value in [
        "",
        "900",
        "x700",
        "900x",
        "0x700",
        "900x0",
        "-900x700",
        "900X700",
        "900x700x1",
        "9e2x700",
        "16385x700",
        " 900x700",
    ] {
        let error = PreviewStart::parse(None, None, Some(value))
            .expect_err("malformed window size is rejected");
        assert!(
            error
                .to_string()
                .contains("EINFACH_MAIL_UI_PREVIEW_WINDOW_SIZE"),
            "{value:?} is rejected with the variable name"
        );
    }
    assert_eq!(
        PreviewStart::parse(None, None, Some("16384x1"))
            .expect("edge limits are inclusive")
            .window_size,
        Some(PreviewWindowSize {
            width: 16_384,
            height: 1,
        })
    );
}
