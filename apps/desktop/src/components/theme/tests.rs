use std::{fs, path::Path};

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, try_parse_color};
use gpui_kit::{Hsla, TestAppContext, WindowAppearance, px};

use super::{StatusTone, install, status_colors};
use crate::features::AppearancePreference;

fn color(hex: &str) -> Hsla {
    try_parse_color(hex).expect("the expected color is a valid hex value")
}

fn installed(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        install(cx);
    });
}

#[derive(Debug, PartialEq)]
struct PaletteSample {
    background: Hsla,
    foreground: Hsla,
    border: Hsla,
    input: Hsla,
    muted_foreground: Hsla,
    primary: Hsla,
    primary_foreground: Hsla,
    ring: Hsla,
    sidebar: Hsla,
    group_box: Hsla,
    popover: Hsla,
    list_active: Hsla,
    list_hover: Hsla,
    secondary_active: Hsla,
    info: Hsla,
    warning: Hsla,
    danger: Hsla,
    success: Hsla,
}

impl PaletteSample {
    fn light() -> Self {
        Self {
            background: color("#F9F9F9"),
            foreground: color("#1B1B1B"),
            border: color("#E5E5E5"),
            input: color("#8A8A8A"),
            muted_foreground: color("#5F5F5F"),
            primary: color("#005FB8"),
            primary_foreground: color("#FFFFFF"),
            ring: color("#00264D"),
            sidebar: color("#F3F3F3"),
            group_box: color("#FFFFFF"),
            popover: color("#FCFCFC"),
            list_active: color("#005FB81F"),
            list_hover: color("#F0F0F0"),
            secondary_active: color("#E3E3E3"),
            info: color("#005FB8"),
            warning: color("#9D5D00"),
            danger: color("#C42B1C"),
            success: color("#0F7B0F"),
        }
    }

    fn dark() -> Self {
        Self {
            background: color("#282828"),
            foreground: color("#FFFFFF"),
            border: color("#3B3B3B"),
            input: color("#9A9A9A"),
            muted_foreground: color("#C8C8C8"),
            primary: color("#60CDFF"),
            primary_foreground: color("#000000"),
            ring: color("#70D3FF"),
            sidebar: color("#202020"),
            group_box: color("#2F2F2F"),
            popover: color("#2C2C2C"),
            list_active: color("#60CDFF24"),
            list_hover: color("#2F2F2F"),
            secondary_active: color("#383838"),
            info: color("#60CDFF"),
            warning: color("#FCE100"),
            danger: color("#FF99A4"),
            success: color("#6CCB5F"),
        }
    }

    fn from_theme(theme: &Theme) -> Self {
        Self {
            background: theme.background,
            foreground: theme.foreground,
            border: theme.border,
            input: theme.input,
            muted_foreground: theme.muted_foreground,
            primary: theme.primary,
            primary_foreground: theme.primary_foreground,
            ring: theme.ring,
            sidebar: theme.sidebar,
            group_box: theme.group_box,
            popover: theme.popover,
            list_active: theme.list_active,
            list_hover: theme.list_hover,
            secondary_active: theme.secondary_active,
            info: theme.info,
            warning: theme.warning,
            danger: theme.danger,
            success: theme.success,
        }
    }
}

fn active_palette(cx: &TestAppContext) -> PaletteSample {
    cx.read(|cx| PaletteSample::from_theme(cx.theme()))
}

#[gpui_kit::test]
fn fluent_theme_values_survive_mode_changes(cx: &mut TestAppContext) {
    installed(cx);

    for (mode, expected) in [
        (ThemeMode::Light, PaletteSample::light()),
        (ThemeMode::Dark, PaletteSample::dark()),
        (ThemeMode::Light, PaletteSample::light()),
        (
            AppearancePreference::System.theme_mode(WindowAppearance::Dark),
            PaletteSample::dark(),
        ),
        (
            AppearancePreference::System.theme_mode(WindowAppearance::Light),
            PaletteSample::light(),
        ),
    ] {
        cx.update(|cx| Theme::change(mode, None, cx));

        assert_eq!(
            active_palette(cx),
            expected,
            "{mode:?} keeps the Fluent palette"
        );
    }
}

#[gpui_kit::test]
fn installing_applies_the_active_mode_without_a_second_change(cx: &mut TestAppContext) {
    installed(cx);

    assert_eq!(active_palette(cx), PaletteSample::light());
    cx.update(|cx| Theme::change(ThemeMode::Dark, None, cx));
    cx.update(install);
    assert_eq!(
        active_palette(cx),
        PaletteSample::dark(),
        "installing again keeps the current mode"
    );
}

#[gpui_kit::test]
fn fluent_shape_tokens_match_the_design(cx: &mut TestAppContext) {
    installed(cx);

    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        cx.update(|cx| Theme::change(mode, None, cx));
        let (radius, radius_lg, font_size) = cx.read(|cx| {
            (
                cx.theme().radius,
                cx.theme().radius_lg,
                cx.theme().font_size,
            )
        });

        assert_eq!(radius, px(4.), "{mode:?} control radius");
        assert_eq!(radius_lg, px(8.), "{mode:?} card radius");
        assert_eq!(font_size, px(16.), "{mode:?} keeps the 16px base font");
    }
}

#[gpui_kit::test]
fn selected_row_fill_stays_translucent_after_gpui_kit_clamps_it(cx: &mut TestAppContext) {
    installed(cx);

    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        cx.update(|cx| Theme::change(mode, None, cx));
        let alpha = cx.read(|cx| cx.theme().list_active.a);

        assert!(
            alpha > 0.05 && alpha <= 0.2,
            "{mode:?} list.active alpha {alpha} is visible and within GPUI Kit's 0.2 clamp"
        );
    }
}

#[gpui_kit::test]
fn status_colors_tint_the_background_and_keep_the_icon_at_full_strength(cx: &mut TestAppContext) {
    installed(cx);

    for (mode, expected) in [
        (ThemeMode::Light, PaletteSample::light()),
        (ThemeMode::Dark, PaletteSample::dark()),
    ] {
        cx.update(|cx| Theme::change(mode, None, cx));
        for (tone, icon) in [
            (StatusTone::Informational, expected.info),
            (StatusTone::Success, expected.success),
            (StatusTone::Warning, expected.warning),
            (StatusTone::Error, expected.danger),
        ] {
            let colors = cx.read(|cx| status_colors(tone, cx));

            assert_eq!(colors.icon, icon, "{mode:?} {tone:?} icon");
            assert_eq!(
                colors.background,
                icon.opacity(0.12),
                "{mode:?} {tone:?} tint"
            );
        }
    }
}

const THEME_FILES: [&str; 2] = ["components/theme.rs", "components/theme"];

#[test]
fn raw_color_literals_live_only_in_the_theme_files() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    collect_color_literals(&source_root, &source_root, &mut violations);

    assert!(
        violations.is_empty(),
        "move these colors into einfach-fluent.json and read them from cx.theme():\n{}",
        violations.join("\n")
    );
}

fn collect_color_literals(source_root: &Path, directory: &Path, violations: &mut Vec<String>) {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .expect("the desktop source directory is readable")
        .map(|entry| {
            entry
                .expect("the source directory entry is readable")
                .path()
        })
        .collect();
    entries.sort();
    for path in entries {
        let relative = path
            .strip_prefix(source_root)
            .expect("every scanned path is under the source root")
            .to_string_lossy()
            .replace('\\', "/");
        if THEME_FILES.contains(&relative.as_str()) {
            continue;
        }
        if path.is_dir() {
            collect_color_literals(source_root, &path, violations);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = fs::read_to_string(&path).expect("the Rust source file is readable");
            for (index, line) in text.lines().enumerate() {
                if let Some(literal) = color_literal(line) {
                    violations.push(format!("{relative}:{}: {literal}", index + 1));
                }
            }
        }
    }
}

/// Returns the first color constructor call or `#RRGGBB` literal on `line`.
fn color_literal(line: &str) -> Option<&str> {
    let boundary = |index: usize| {
        line[..index]
            .chars()
            .next_back()
            .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
    };
    for constructor in ["rgb(", "rgba(", "hsla(", "hsl("] {
        for (index, _) in line.match_indices(constructor) {
            if boundary(index) {
                return Some(constructor);
            }
        }
    }
    line.match_indices('#').find_map(|(index, _)| {
        let digits = line[index + 1..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        matches!(digits, 6 | 8).then_some("#RRGGBB")
    })
}

#[test]
fn the_scan_recognizes_each_forbidden_literal_and_ignores_helper_names() {
    for line in [
        "let color = rgb(0x005fb8);",
        "let color = gpui_kit::rgba(0x005fb8ff);",
        "background(hsla(0., 0., 0., 1.))",
        "let hue = hsl(210., 1., 0.4);",
        "const ACCENT: &str = \"#005FB8\";",
        "const ACCENT: &str = \"#005fb8ff\";",
    ] {
        assert!(color_literal(line).is_some(), "{line} is flagged");
    }
    for line in [
        "let rgb = background.to_rgb();",
        "relative_luminance(rgb_value)",
        "let tokens = Hsla::default();",
        "// see issue #4521 for the card layout",
        "let anchor = \"#abc\";",
    ] {
        assert!(color_literal(line).is_none(), "{line} is not flagged");
    }
}
