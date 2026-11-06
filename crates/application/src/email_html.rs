//! Sanitized message-body values prepared for native display.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use ammonia::{Builder, UrlRelative};
use einfach_mail_domain::{MessageBody, MessageBodyType};
use url::Url;

const MAX_HTML_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_HTML_FALLBACK_INPUT_BYTES: usize = MAX_HTML_BODY_BYTES / 8;
// Ammonia can replace one-byte NUL characters with the eight-byte entity `&#65533;`.
const MAX_HTML_FALLBACK_TEXT_BYTES: usize = MAX_HTML_BODY_BYTES / 128;
const HTML_FALLBACK_WRAP_WIDTH: usize = 80;

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

/// A message body after the application applies its native-display policy.
///
/// HTML bodies are sanitized, converted to escaped plain text, or replaced
/// with a size-limit message. No variant exposes unsanitized HTML to a view.
#[derive(Clone, Eq, PartialEq)]
pub enum MessageBodyForDisplay {
    /// Plain text content retained from the provider body.
    PlainText(Arc<str>),
    /// HTML that passed the application's allowlist sanitizer.
    SanitizedHtml(SanitizedEmailHtml),
    /// An escaped plain-text excerpt from sanitized HTML above the rich-render limit.
    PlainTextFallback(SanitizedEmailHtml),
    /// HTML that exceeded the limit accepted by the sanitizer.
    HtmlTooLarge,
}

impl fmt::Debug for MessageBodyForDisplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlainText(_) => formatter.write_str("PlainText(<redacted>)"),
            Self::SanitizedHtml(_) => formatter.write_str("SanitizedHtml(<redacted>)"),
            Self::PlainTextFallback(_) => formatter.write_str("PlainTextFallback(<redacted>)"),
            Self::HtmlTooLarge => formatter.write_str("HtmlTooLarge"),
        }
    }
}

/// HTML sanitized for display by GPUI Kit's native text renderer.
///
/// Its private field prevents callers from constructing this type from raw
/// provider or cache content.
#[derive(Clone, Eq, PartialEq)]
pub struct SanitizedEmailHtml(Arc<str>);

impl SanitizedEmailHtml {
    /// Returns the sanitized HTML fragment.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Clones the immutable content without copying its bytes.
    #[must_use]
    pub fn shared_content(&self) -> Arc<str> {
        Arc::clone(&self.0)
    }
}

impl fmt::Debug for SanitizedEmailHtml {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SanitizedEmailHtml(<redacted>)")
    }
}

/// A validated HTTP or HTTPS destination from a message link.
#[derive(Clone, Eq, PartialEq)]
pub struct ExternalMessageLink(Box<str>);

impl ExternalMessageLink {
    /// Returns the normalized destination that passed message-link policy.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ExternalMessageLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExternalMessageLink(<redacted>)")
    }
}

/// Applies the native display policy to a raw cached or provider body.
///
/// Call this after storing provider content so the cache always retains the
/// original body and a newer policy is applied whenever the message is opened.
#[must_use]
pub fn prepare_message_body_for_display(body: &MessageBody) -> MessageBodyForDisplay {
    match body.content_type() {
        MessageBodyType::Text => MessageBodyForDisplay::PlainText(Arc::from(body.content())),
        MessageBodyType::Html if body.content().len() > MAX_HTML_BODY_BYTES => {
            MessageBodyForDisplay::HtmlTooLarge
        }
        MessageBodyType::Html => prepare_html_for_display(body.content()),
    }
}

fn prepare_html_for_display(raw_html: &str) -> MessageBodyForDisplay {
    let sanitized_html = sanitize_html(raw_html);
    if sanitized_html.len() <= MAX_HTML_BODY_BYTES {
        return MessageBodyForDisplay::SanitizedHtml(SanitizedEmailHtml(Arc::from(sanitized_html)));
    }

    plain_text_fallback(&sanitized_html).map_or(
        MessageBodyForDisplay::HtmlTooLarge,
        MessageBodyForDisplay::PlainTextFallback,
    )
}

/// Parses and validates a message link before the desktop opens it externally.
///
/// Returns a link only when it is absolute HTTP or HTTPS and contains no user
/// information, so a username cannot appear before the actual host.
#[must_use]
pub fn allowed_external_message_url(href: &str) -> Option<ExternalMessageLink> {
    let url = Url::parse(href).ok()?;
    let is_web_url = matches!(url.scheme(), "http" | "https");
    let has_no_user_info = url.username().is_empty() && url.password().is_none();
    (is_web_url && has_no_user_info).then(|| ExternalMessageLink(url.as_str().into()))
}

fn sanitize_html(raw_html: &str) -> String {
    let mut builder = Builder::default();
    builder
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .tag_attributes(HashMap::from([("a", HashSet::from(["href"]))]))
        .generic_attributes(HashSet::new())
        .url_schemes(HashSet::from(["http", "https"]))
        .url_relative(UrlRelative::Deny)
        .clean_content_tags(REMOVE_CONTENT_TAGS.iter().copied().collect());
    builder.clean(raw_html).to_string()
}

fn plain_text_fallback(sanitized_html: &str) -> Option<SanitizedEmailHtml> {
    let bounded_html = bounded_prefix(sanitized_html, MAX_HTML_FALLBACK_INPUT_BYTES);
    let plain_text = html2text::config::plain_no_decorate()
        .allow_width_overflow()
        .string_from_read(bounded_html.as_bytes(), HTML_FALLBACK_WRAP_WIDTH)
        .ok()?;
    let plain_text = bounded_prefix(&plain_text, MAX_HTML_FALLBACK_TEXT_BYTES);
    if plain_text.trim().is_empty() {
        return None;
    }

    let escaped_text = ammonia::clean_text(plain_text);
    let safe_html = format!("<pre>{escaped_text}</pre>");
    Some(SanitizedEmailHtml(Arc::from(safe_html)))
}

fn bounded_prefix(content: &str, max_bytes: usize) -> &str {
    let mut end = content.len().min(max_bytes);
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    &content[..end]
}

#[cfg(test)]
mod tests {
    use einfach_mail_domain::{MessageBody, MessageBodyType};

    use super::{
        MAX_HTML_BODY_BYTES, MAX_HTML_FALLBACK_TEXT_BYTES, MessageBodyForDisplay,
        allowed_external_message_url, prepare_message_body_for_display,
    };

    fn sanitized_html_for_test(raw_html: &str) -> String {
        let body = MessageBody::new(MessageBodyType::Html, raw_html.to_owned());
        let MessageBodyForDisplay::SanitizedHtml(html) = prepare_message_body_for_display(&body)
        else {
            panic!("synthetic HTML remains within the rich-render limit");
        };
        html.as_str().to_owned()
    }

    #[test]
    fn sanitizes_active_html_and_retains_common_formatting() {
        let body = MessageBody::new(
            MessageBodyType::Html,
            concat!(
                "<p onclick=\"run()\"><strong>safe</strong><a href=\"https://mail.example.test\">link</a><img src=\"https://tracker.example.test/pixel\"></p>",
                "<ul><li>first</li><li>second</li></ul>",
                "<table><tbody><tr><th>key</th><td>value</td></tr></tbody></table>",
                "<script>secret marker</script>"
            )
            .to_owned(),
        );

        let MessageBodyForDisplay::SanitizedHtml(html) = prepare_message_body_for_display(&body)
        else {
            panic!("normal-sized HTML is sanitized");
        };

        assert!(html.as_str().contains("<strong>safe</strong>"));
        assert!(html.as_str().contains("href=\"https://mail.example.test\""));
        assert!(
            html.as_str()
                .contains("<ul><li>first</li><li>second</li></ul>")
        );
        assert!(html.as_str().contains("<th>key</th>"));
        assert!(html.as_str().contains("<td>value</td>"));
        assert!(!html.as_str().contains("onclick"));
        assert!(!html.as_str().contains("tracker.example.test"));
        assert!(!html.as_str().contains("secret marker"));
    }

    #[test]
    fn removes_active_content_remote_resources_and_unapproved_attributes() {
        let sanitized = sanitized_html_for_test(
            r#"
                <p class="mail" id="content" lang="en" title="Paragraph" style="color:red"
                   onclick="run()" onmouseover="run()">readable</p>
                <script>script marker</script>
                <style>@import url(https://tracker.invalid/style.css);</style>
                <img src="https://tracker.invalid/image.png" alt="image marker">
                <a href="https://mail.example.invalid" target="_blank" onfocus="focus marker">safe label</a>
                <link rel="stylesheet" href="https://tracker.invalid/linked.css">
                <meta http-equiv="refresh" content="0;url=https://tracker.invalid/refresh">
                <base href="https://tracker.invalid/">
                <iframe src="https://tracker.invalid/frame">frame marker</iframe>
                <form action="https://tracker.invalid/submit"><input value="form marker"><button>submit marker</button></form>
                <object data="https://tracker.invalid/object">object marker</object>
                <embed src="https://tracker.invalid/embed">
                <video><source src="https://tracker.invalid/video"></video>
                <audio><source src="https://tracker.invalid/audio"></audio>
                <p>after</p>
            "#,
        );

        for forbidden in [
            "class=",
            "id=",
            "lang=",
            "title=",
            "style=",
            "onclick",
            "onmouseover",
            "onfocus",
            "target=",
            "tracker.invalid",
            "script marker",
            "image marker",
            "frame marker",
            "form marker",
            "focus marker",
            "submit marker",
            "object marker",
            "<script",
            "<style",
            "<img",
            "<link",
            "<meta",
            "<base",
            "<iframe",
            "<form",
            "<object",
            "<embed",
            "<video",
            "<audio",
        ] {
            assert!(
                !sanitized.contains(forbidden),
                "unexpected sanitized fragment: {forbidden}"
            );
        }
        assert!(sanitized.contains("readable"));
        assert!(sanitized.contains("safe label"));
        assert!(sanitized.contains("after"));
    }

    #[test]
    fn removes_svg_mathml_and_css_url_variants() {
        let sanitized = sanitized_html_for_test(
            r#"
                <p>before</p>
                <svg><foreignObject><p>svg mutation marker</p><math><mi>nested marker</mi></math></foreignObject></svg>
                <math><mtext><svg><desc>math mutation marker</desc></svg></mtext></math>
                <p style="background:url(https://tracker.invalid/lower)">lower</p>
                <p style="background:URL(&quot;https://tracker.invalid/upper&quot;)">upper</p>
                <p style="background:Url(https://tracker.invalid/mixed)">mixed</p>
                <style>p { background: URL(https://tracker.invalid/block) }</style>
                <p>after</p>
            "#,
        );

        for forbidden in [
            "svg mutation marker",
            "nested marker",
            "math mutation marker",
            "<svg",
            "<math",
            "style=",
            "<style",
            "tracker.invalid",
        ] {
            assert!(
                !sanitized.contains(forbidden),
                "unexpected sanitized fragment: {forbidden}"
            );
        }
        assert!(sanitized.contains("before"));
        assert!(sanitized.contains("lower"));
        assert!(sanitized.contains("upper"));
        assert!(sanitized.contains("mixed"));
        assert!(sanitized.contains("after"));
    }

    #[test]
    fn sanitizes_malformed_html_to_a_stable_fragment() {
        let sanitized = sanitized_html_for_test("<p>first <strong>bold</p>second <em>italic");

        assert!(sanitized.contains("first"));
        assert!(sanitized.contains("bold"));
        assert!(sanitized.contains("second"));
        assert!(sanitized.contains("italic"));
        assert_eq!(sanitized, sanitized_html_for_test(&sanitized));
    }

    #[test]
    fn removes_unsafe_and_relative_link_targets() {
        for href in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "java&#x09;script:alert(1)",
            "data:text/html,unsafe",
            "vbscript:unsafe",
            "file:///etc/passwd",
            "mailto:person@example.invalid",
            "/relative/path",
            "//tracker.invalid/path",
        ] {
            let sanitized = sanitized_html_for_test(&format!("<a href=\"{href}\">link text</a>"));

            assert!(
                !sanitized.contains("href="),
                "unsafe link target {href:?} survived: {sanitized}"
            );
            assert!(sanitized.contains("link text"));
        }
    }

    #[test]
    fn rejects_html_above_the_provider_response_budget() {
        let oversized_html = "x".repeat(MAX_HTML_BODY_BYTES + 1);
        let body = MessageBody::new(MessageBodyType::Html, oversized_html);

        assert_eq!(
            prepare_message_body_for_display(&body),
            MessageBodyForDisplay::HtmlTooLarge
        );
    }

    #[test]
    fn sanitizes_a_large_html_body_within_the_provider_response_budget() {
        let large_html = format!("<p>{}</p>", "x".repeat(1024 * 1024));
        let body = MessageBody::new(MessageBodyType::Html, large_html);

        let MessageBodyForDisplay::SanitizedHtml(html) = prepare_message_body_for_display(&body)
        else {
            panic!("one-megabyte HTML remains eligible for sanitization");
        };

        assert_eq!(html.as_str().len(), 1024 * 1024 + "<p></p>".len());
    }

    #[test]
    fn converts_oversized_sanitized_html_to_bounded_escaped_plain_text() {
        let expanded_text = ">".repeat(MAX_HTML_BODY_BYTES / 4);
        let raw_html = format!(
            "<p>Readable fallback text <a href=\"https://links.example.test\">link label</a> {expanded_text}</p><script>hidden script marker</script>"
        );
        assert!(raw_html.len() <= MAX_HTML_BODY_BYTES);
        let body = MessageBody::new(MessageBodyType::Html, raw_html);
        let display_body = prepare_message_body_for_display(&body);
        assert!(!format!("{display_body:?}").contains("Readable fallback text"));

        let MessageBodyForDisplay::PlainTextFallback(content) = display_body else {
            panic!("expanded sanitized HTML uses a plain-text fallback");
        };

        assert!(content.as_str().starts_with("<pre>"));
        assert!(!content.as_str().contains("<a "));
        assert!(!content.as_str().contains("links.example.test"));
        assert!(!content.as_str().contains("hidden script marker"));
        assert!(content.as_str().len() <= MAX_HTML_BODY_BYTES / 16);

        let rendered = html2text::config::plain_no_decorate()
            .allow_width_overflow()
            .string_from_read(content.as_str().as_bytes(), 80)
            .expect("escaped fallback HTML is readable");
        assert!(rendered.contains("Readable fallback text [link label]"));
        assert!(!rendered.contains("hidden script marker"));
        assert!(rendered.len() <= MAX_HTML_FALLBACK_TEXT_BYTES + 8 * 1024);
    }

    #[test]
    fn bounds_utf8_fallback_prefixes_at_character_boundaries() {
        let prefix = super::bounded_prefix("abc€", 5);

        assert_eq!(prefix, "abc");
    }

    #[test]
    fn preserves_plain_text_without_interpreting_markup() {
        let body = MessageBody::new(
            MessageBodyType::Text,
            "<script>readable text</script>".to_owned(),
        );

        let MessageBodyForDisplay::PlainText(text) = prepare_message_body_for_display(&body) else {
            panic!("plain text remains plain text");
        };

        assert_eq!(text.as_ref(), "<script>readable text</script>");
    }

    #[test]
    fn redacts_message_content_from_debug_output() {
        let body = MessageBody::new(MessageBodyType::Html, "private synthetic marker".to_owned());
        let display_body = prepare_message_body_for_display(&body);

        assert!(!format!("{display_body:?}").contains("private synthetic marker"));
    }

    #[test]
    fn allows_only_absolute_http_and_https_links_without_user_info() {
        for href in [
            "http://mail.example.test/thread",
            "https://mail.example.test/thread",
        ] {
            assert!(allowed_external_message_url(href).is_some());
        }

        for href in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "java&#x09;script:alert(1)",
            "data:text/html,unsafe",
            "mailto:person@example.test",
            "file:///etc/passwd",
            "/relative/path",
            "//tracker.example.test/path",
            "https://person@mail.example.test/path",
        ] {
            assert!(
                allowed_external_message_url(href).is_none(),
                "accepted {href:?}"
            );
        }
    }

    #[test]
    fn normalized_external_links_do_not_reveal_the_destination_in_debug_output() {
        let link =
            allowed_external_message_url("HTTPS://mail.example.test/thread?token=synthetic-secret")
                .expect("synthetic HTTPS URL is allowed");

        assert_eq!(
            link.as_str(),
            "https://mail.example.test/thread?token=synthetic-secret"
        );
        assert!(!format!("{link:?}").contains("synthetic-secret"));
    }
}
