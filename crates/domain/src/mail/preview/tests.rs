use super::MessagePreview;

fn preview_text(input: &str) -> Option<String> {
    MessagePreview::from_untrusted(input).map(|preview| preview.as_str().to_owned())
}

#[test]
fn plain_text_is_kept() {
    assert_eq!(
        preview_text("Synthetic opening line"),
        Some("Synthetic opening line".to_owned())
    );
}

#[test]
fn control_characters_become_a_space() {
    assert_eq!(
        preview_text("one\ttwo\nthree\r\nfour\u{0}five\u{7F}six\u{85}seven"),
        Some("one two three four five six seven".to_owned())
    );
}

#[test]
fn line_and_paragraph_separators_become_a_space() {
    assert_eq!(
        preview_text("one\u{2028}two\u{2029}three"),
        Some("one two three".to_owned())
    );
}

#[test]
fn unicode_whitespace_becomes_a_space() {
    assert_eq!(
        preview_text("one\u{A0}two\u{2003}three\u{3000}four\u{202F}five"),
        Some("one two three four five".to_owned())
    );
}

#[test]
fn bidirectional_controls_are_dropped() {
    let controls = [
        '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}',
        '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ];
    for control in controls {
        assert_eq!(
            preview_text(&format!("ab{control}cd")),
            Some("abcd".to_owned()),
            "U+{:04X} was kept",
            u32::from(control)
        );
    }
}

#[test]
fn invisible_padding_is_dropped() {
    let padding = [
        '\u{00AD}', '\u{034F}', '\u{200B}', '\u{200C}', '\u{2060}', '\u{FEFF}',
    ];
    for character in padding {
        assert_eq!(
            preview_text(&format!("ab{character}cd")),
            Some("abcd".to_owned()),
            "U+{:04X} was kept",
            u32::from(character)
        );
    }
}

#[test]
fn dropped_characters_between_spaces_do_not_split_a_space_run() {
    assert_eq!(
        preview_text("one \u{200B}\u{FEFF} \u{202E} two"),
        Some("one two".to_owned())
    );
}

#[test]
fn whitespace_runs_collapse_and_the_result_is_trimmed() {
    assert_eq!(
        preview_text(" \t\n one   two \u{A0}\u{A0} three \n\n"),
        Some("one two three".to_owned())
    );
}

#[test]
fn input_without_printable_characters_has_no_preview() {
    for input in [
        "",
        " \t\r\n",
        "\u{A0}\u{2003}\u{3000}",
        "\u{200B}\u{FEFF}\u{202E}\u{AD}",
        " \u{200B} \u{0} ",
    ] {
        assert_eq!(preview_text(input), None, "{input:?} produced a preview");
    }
}

#[test]
fn text_at_the_cap_is_kept_whole() {
    let input = "a".repeat(MessagePreview::MAX_CHARS);

    assert_eq!(preview_text(&input), Some(input));
}

#[test]
fn text_over_the_cap_is_cut_at_the_cap() {
    let input = "a".repeat(MessagePreview::MAX_CHARS + 50);

    assert_eq!(
        preview_text(&input),
        Some("a".repeat(MessagePreview::MAX_CHARS))
    );
}

#[test]
fn the_cap_counts_characters_not_bytes() {
    let input = "é".repeat(MessagePreview::MAX_CHARS + 1);

    let preview = preview_text(&input).expect("input has printable characters");

    assert_eq!(preview.chars().count(), MessagePreview::MAX_CHARS);
}

#[test]
fn the_cap_counts_output_characters_not_input_characters() {
    let input = format!("{}{}", "\u{200B}".repeat(1_000), "a".repeat(10));

    assert_eq!(preview_text(&input), Some("a".repeat(10)));
}

#[test]
fn a_cut_never_leaves_a_trailing_space() {
    let input = format!("{} tail", "a".repeat(MessagePreview::MAX_CHARS - 1));

    let preview = preview_text(&input).expect("input has printable characters");

    assert_eq!(preview, "a".repeat(MessagePreview::MAX_CHARS - 1));
}

#[test]
fn a_cut_after_a_word_keeps_the_word_boundary_space_that_fits() {
    let input = format!("{} b tail", "a".repeat(MessagePreview::MAX_CHARS - 3));

    let preview = preview_text(&input).expect("input has printable characters");

    assert_eq!(
        preview,
        format!("{} b", "a".repeat(MessagePreview::MAX_CHARS - 3))
    );
    assert_eq!(preview.chars().count(), MessagePreview::MAX_CHARS - 1);
}

#[test]
fn normalizing_a_preview_again_returns_the_same_preview() {
    let long_with_spaces = "word ".repeat(200);
    let samples = [
        "",
        "plain",
        "  padded  ",
        "a\tb\nc",
        "one\u{2028}two\u{2029}three",
        "\u{202E}reversed\u{202C}",
        "pre\u{200B}\u{FEFF}header \u{34F}\u{AD} text",
        "\u{A0}\u{3000}only spaces",
        "emoji 😀 and é and 日本語",
        &long_with_spaces,
        &"a".repeat(MessagePreview::MAX_CHARS + 20),
        &format!("{} b tail", "a".repeat(MessagePreview::MAX_CHARS - 3)),
        &format!("{} tail", "a".repeat(MessagePreview::MAX_CHARS - 1)),
        &format!("{}\u{200B} x", "a".repeat(MessagePreview::MAX_CHARS - 2)),
    ];

    for sample in samples {
        let Some(first) = MessagePreview::from_untrusted(sample) else {
            continue;
        };
        let second = MessagePreview::from_untrusted(first.as_str());

        assert_eq!(second.as_ref(), Some(&first), "{sample:?} was not stable");
    }
}

#[test]
fn every_preview_stays_within_its_invariants() {
    let samples = [
        "plain",
        "\u{202E}a\u{200B} \t b\u{2028}",
        &"x y ".repeat(300),
        &"\u{A0}a".repeat(300),
    ];

    for sample in samples {
        let preview = preview_text(sample).expect("sample has printable characters");

        assert!(preview.chars().count() <= MessagePreview::MAX_CHARS);
        assert!(!preview.starts_with(' ') && !preview.ends_with(' '));
        assert!(!preview.contains("  "));
        assert!(preview.chars().all(|character| !character.is_control()
            && !character.is_whitespace()
            || character == ' '));
    }
}

#[test]
fn debug_output_redacts_the_text() {
    let preview = MessagePreview::from_untrusted("preview-text-marker")
        .expect("synthetic preview has printable text");

    assert_eq!(format!("{preview:?}"), "MessagePreview(<redacted>)");
}
