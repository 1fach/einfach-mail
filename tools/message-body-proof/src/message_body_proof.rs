//! Synthetic tests for the proposed email HTML sanitization policy.

use ammonia::{Builder, UrlRelative};
use std::collections::{HashMap, HashSet};

const ALLOWED_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "caption",
    "code",
    "dd",
    "del",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "ins",
    "li",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "small",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

const CLEAN_CONTENT_TAGS: &[&str] = &[
    "audio",
    "button",
    "canvas",
    "embed",
    "form",
    "iframe",
    "math",
    "noembed",
    "noframes",
    "noscript",
    "object",
    "plaintext",
    "script",
    "select",
    "style",
    "svg",
    "template",
    "textarea",
    "video",
    "xmp",
];

fn sanitizer() -> Builder<'static> {
    let mut builder = Builder::empty();
    builder.tags(ALLOWED_TAGS.iter().copied().collect());
    builder.tag_attributes(HashMap::from([("a", HashSet::from(["href", "title"]))]));
    builder.generic_attributes(HashSet::new());
    builder.clean_content_tags(CLEAN_CONTENT_TAGS.iter().copied().collect());
    builder.url_schemes(HashSet::from(["http", "https"]));
    builder.url_relative(UrlRelative::Deny);
    builder.link_rel(Some("noopener noreferrer"));
    builder
}

fn sanitize(html: &str) -> String {
    sanitizer().clean(html).to_string()
}

fn assert_absent(output: &str, value: &str) {
    assert!(
        !output.contains(value),
        "sanitized output retained {value:?}: {output}"
    );
}

#[test]
fn preserves_common_message_formatting_and_web_links() {
    let output = sanitize(
        r#"
        <p>Hello <strong>team</strong>, <em>please review</em> this message.</p>
        <ol><li>First</li><li>Second</li></ol>
        <table><thead><tr><th>Key</th></tr></thead>
            <tbody><tr><td>Value</td></tr></tbody></table>
        <a href="https://example.org/path" title="Details">Read more</a>
        <a href="http://example.org/legacy">Older link</a>
        "#,
    );

    for expected in [
        "<p>",
        "<strong>",
        "<em>",
        "<ol>",
        "<li>",
        "<table>",
        "<th>",
        "<td>",
        "href=\"https://example.org/path\"",
        "title=\"Details\"",
        "href=\"http://example.org/legacy\"",
        "rel=\"noopener noreferrer\"",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in {output}"
        );
    }
}

#[test]
fn removes_active_content_and_remote_resources() {
    let output = sanitize(
        r#"
        <script>script-payload</script>
        <style>
            @import url(https://tracker.example/style.css);
            .hidden { background: URL(https://tracker.example/pixel) }
            .also-hidden { background: uRl("https://tracker.example/other-pixel") }
        </style>
        <img src="https://tracker.example/image" onerror="image-payload">
        <iframe src="https://tracker.example/frame">frame-payload</iframe>
        <object data="https://tracker.example/object">object-payload</object>
        <embed src="https://tracker.example/embed">
        <svg><script>svg-payload</script><image href="https://tracker.example/svg" /></svg>
        <svg><foreignObject><table><path><xmp><!--</xmp><img
            src="https://tracker.example/mutation" onerror="mutation-payload"></foreignObject></svg>
        <math><mi>math-payload</mi></math>
        <math><annotation-xml encoding="text/html"><xmp><!--</xmp><img
            src="https://tracker.example/math-mutation" onerror="math-mutation-payload"></annotation-xml></math>
        <video><source src="https://tracker.example/video">video-payload</video>
        <audio src="https://tracker.example/audio">audio-payload</audio>
        <form action="https://tracker.example/submit"><button>form-payload</button></form>
        "#,
    );

    for forbidden in [
        "<script",
        "script-payload",
        "<style",
        "tracker.example",
        "image-payload",
        "frame-payload",
        "object-payload",
        "svg-payload",
        "mutation-payload",
        "math-payload",
        "math-mutation-payload",
        "video-payload",
        "audio-payload",
        "form-payload",
        "<svg",
        "<math",
        "<img",
        "<iframe",
        "<form",
    ] {
        assert_absent(&output, forbidden);
    }
}

#[test]
fn removes_event_style_and_navigation_attributes() {
    let output = sanitize(
        r#"
        <p class="mail" id="content" lang="en" title="Paragraph"
           style="color:red; background:URL(https://tracker.example/pixel); background:uRl(https://tracker.example/other)"
           onclick="event-payload" onmouseover="hover-payload">Text</p>
        <a href="https://example.org" target="_blank" onfocus="focus-payload">Link</a>
        <link rel="stylesheet" href="https://tracker.example/style.css">
        <meta http-equiv="refresh" content="0;url=https://tracker.example/refresh">
        <base href="https://tracker.example/">
        "#,
    );

    for forbidden in [
        "class=",
        "id=",
        "lang=",
        "title=\"Paragraph\"",
        "style=",
        "onclick",
        "onmouseover",
        "onfocus",
        "target=",
        "event-payload",
        "hover-payload",
        "focus-payload",
        "tracker.example",
        "<link",
        "<meta",
        "<base",
    ] {
        assert_absent(&output, forbidden);
    }
}

#[test]
fn retains_only_absolute_http_and_https_link_targets() {
    let output = sanitize(
        r#"
        <a href="https://safe.example/one">HTTPS</a>
        <a href="http://safe.example/two">HTTP</a>
        <a href="javascript:alert(1)">JavaScript</a>
        <a href="JaVaScRiPt:alert(2)">Mixed case JavaScript</a>
        <a href="java&#x73;cript:alert(3)">Entity encoded JavaScript</a>
        <a href="vbscript:msgbox(1)">VBScript</a>
        <a href="file:///etc/passwd">File URL</a>
        <a href="data:text/html,unsafe">Data</a>
        <a href="mailto:person@example.org">Mail</a>
        <a href="//tracker.example/path">Protocol-relative</a>
        <a href="/relative/path">Relative</a>
        "#,
    );

    assert!(output.contains("href=\"https://safe.example/one\""));
    assert!(output.contains("href=\"http://safe.example/two\""));
    assert_absent(&output, "javascript:");
    assert_absent(&output, "vbscript:");
    assert_absent(&output, "file:");
    assert_absent(&output, "data:");
    assert_absent(&output, "mailto:");
    assert_absent(&output, "tracker.example");
    assert_absent(&output, "href=\"/relative/path\"");
}

#[test]
fn sanitization_is_stable_for_malformed_html() {
    let first = sanitize(
        "<p><b>Visible text</p><svg><foreignObject><img src=x onerror=payload></foreignObject></svg><script>secret</script>",
    );
    let second = sanitize(&first);

    assert_eq!(second, first);
    assert!(first.contains("Visible text"));
    assert_absent(&first, "foreignObject");
    assert_absent(&first, "onerror");
    assert_absent(&first, "secret");
}
