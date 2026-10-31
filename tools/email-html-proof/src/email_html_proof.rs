use std::collections::{HashMap, HashSet};

use ammonia::{Builder, UrlRelative};

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
    "kbd",
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

const REMOVE_CONTENT_TAGS: &[&str] = &[
    "applet",
    "audio",
    "button",
    "canvas",
    "embed",
    "fieldset",
    "form",
    "frame",
    "frameset",
    "iframe",
    "input",
    "math",
    "noembed",
    "noframes",
    "noscript",
    "object",
    "option",
    "plaintext",
    "script",
    "select",
    "source",
    "style",
    "svg",
    "template",
    "textarea",
    "track",
    "video",
    "xmp",
];

const ACTIVE_CONTENT_FIXTURE: &str = r#"
    <p onclick="run()" onmouseover="run()" style="background:url(https://tracker.invalid/pixel)">readable</p>
    <script>script marker</script>
    <style>@import url(https://tracker.invalid/style.css);</style>
    <img src="https://tracker.invalid/image.png" alt="image marker">
    <link rel="stylesheet" href="https://tracker.invalid/linked.css">
    <iframe src="https://tracker.invalid/frame">frame marker</iframe>
    <form action="https://tracker.invalid/submit"><input value="form marker"><button>submit marker</button></form>
    <object data="https://tracker.invalid/object">object marker</object>
    <embed src="https://tracker.invalid/embed">
    <video><source src="https://tracker.invalid/video"></video>
    <audio><source src="https://tracker.invalid/audio"></audio>
    <p>after</p>
"#;

const ACTIVE_CONTENT_FORBIDDEN_FRAGMENTS: &[&str] = &[
    "onclick",
    "onmouseover",
    "style=",
    "tracker.invalid",
    "script marker",
    "frame marker",
    "form marker",
    "submit marker",
    "object marker",
    "<script",
    "<style",
    "<img",
    "<iframe",
    "<form",
    "<object",
    "<embed",
    "<video",
    "<audio",
];

const COMMON_FORMATTING_FIXTURE: &str = r#"
    <p>Hello <strong>bold</strong> and <em>italic</em>;
        <a href="https://mail.example.invalid/thread">open thread</a>.</p>
    <ul><li>first</li><li>second</li></ul>
    <table><tbody><tr><th>key</th><td>value</td></tr></tbody></table>
"#;

const UNSAFE_LINKS: &[&str] = &[
    "javascript:alert(1)",
    "JaVaScRiPt:alert(1)",
    "java&#x09;script:alert(1)",
    "data:text/html,unsafe",
    "vbscript:unsafe",
    "file:///etc/passwd",
    "mailto:person@example.invalid",
    "/relative/path",
    "//tracker.invalid/path",
];

const NAMESPACE_CONFUSION_FIXTURE: &str = r"
    <p>before</p>
    <svg><foreignObject><p>svg mutation marker</p><math><mi>nested marker</mi></math></foreignObject>
    <math><mtext><svg><desc>math mutation marker</desc></svg></mtext>
    <p>after</p>
";

const CSS_URL_FIXTURE: &str = r#"
    <p style="background:url(https://tracker.invalid/lower)">lower</p>
    <p style="background:URL(&quot;https://tracker.invalid/upper&quot;)">upper</p>
    <p style="background:Url(https://tracker.invalid/mixed)">mixed</p>
    <style>p { background: URL(https://tracker.invalid/block) }</style>
"#;

fn sanitize(source: &str) -> String {
    let mut builder = Builder::default();
    builder
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .tag_attributes(HashMap::from([("a", HashSet::from(["href"]))]))
        .generic_attributes(HashSet::new())
        .url_schemes(HashSet::from(["http", "https"]))
        .url_relative(UrlRelative::Deny)
        .clean_content_tags(REMOVE_CONTENT_TAGS.iter().copied().collect());

    builder.clean(source).to_string()
}

fn assert_absent(sanitized: &str, forbidden_fragments: &[&str]) {
    for fragment in forbidden_fragments {
        assert!(
            !sanitized.contains(fragment),
            "unexpected sanitized fragment: {fragment}"
        );
    }
}

#[test]
fn preserves_common_message_formatting_and_web_links() {
    let sanitized = sanitize(COMMON_FORMATTING_FIXTURE);

    assert!(sanitized.contains("<strong>bold</strong>"));
    assert!(sanitized.contains("<em>italic</em>"));
    assert!(sanitized.contains("href=\"https://mail.example.invalid/thread\""));
    assert!(sanitized.contains("<ul><li>first</li><li>second</li></ul>"));
    assert!(sanitized.contains("<table>"));
    assert!(sanitized.contains("<th>key</th>"));
    assert!(sanitized.contains("<td>value</td>"));
}

#[test]
fn strips_active_and_remote_resource_content() {
    let sanitized = sanitize(ACTIVE_CONTENT_FIXTURE);
    assert_absent(&sanitized, ACTIVE_CONTENT_FORBIDDEN_FRAGMENTS);
    assert!(sanitized.contains("readable"));
    assert!(sanitized.contains("after"));
}

#[test]
fn strips_unsafe_and_relative_link_targets() {
    for href in UNSAFE_LINKS {
        let html = format!("<a href=\"{href}\">link text</a>");
        let sanitized = sanitize(&html);

        assert!(
            !sanitized.contains("href="),
            "unsafe link target {href:?} survived: {sanitized}"
        );
        assert!(sanitized.contains("link text"));
    }
}

#[test]
fn strips_svg_and_mathml_subtrees_from_malformed_markup() {
    let sanitized = sanitize(NAMESPACE_CONFUSION_FIXTURE);

    assert!(sanitized.contains("before"));
    assert!(sanitized.contains("after"));
    assert_absent(
        &sanitized,
        &[
            "svg mutation marker",
            "nested marker",
            "math mutation marker",
            "<svg",
            "<math",
        ],
    );
}

#[test]
fn removes_css_url_variants_with_disallowed_style_markup() {
    let sanitized = sanitize(CSS_URL_FIXTURE);

    assert_absent(&sanitized, &["style=", "<style", "tracker.invalid"]);
    assert!(sanitized.contains("lower"));
    assert!(sanitized.contains("upper"));
    assert!(sanitized.contains("mixed"));
}

#[test]
fn repairs_malformed_html_to_a_stable_fragment() {
    let sanitized = sanitize("<p>first <strong>bold</p>second <em>italic");

    assert!(sanitized.contains("first"));
    assert!(sanitized.contains("bold"));
    assert!(sanitized.contains("second"));
    assert!(sanitized.contains("italic"));
    assert_eq!(sanitized, sanitize(&sanitized));
}
