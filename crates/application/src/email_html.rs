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
/// HTML bodies are either sanitized or replaced with a size-limit message.
/// This value has no variant that exposes unsanitized HTML to a view.
#[derive(Clone, Eq, PartialEq)]
pub enum MessageBodyForDisplay {
    /// Plain text content retained from the provider body.
    PlainText(Arc<str>),
    /// HTML that passed the application's allowlist sanitizer.
    SanitizedHtml(SanitizedEmailHtml),
    /// HTML that exceeded the limit accepted by the sanitizer.
    HtmlTooLarge,
}

impl fmt::Debug for MessageBodyForDisplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlainText(_) => formatter.write_str("PlainText(<redacted>)"),
            Self::SanitizedHtml(_) => formatter.write_str("SanitizedHtml(<redacted>)"),
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
        MessageBodyType::Html => sanitize_html(body.content()).map_or(
            MessageBodyForDisplay::HtmlTooLarge,
            MessageBodyForDisplay::SanitizedHtml,
        ),
    }
}

/// Parses and validates a message link before the desktop opens it externally.
///
/// Returns a link only when it is absolute HTTP or HTTPS and contains no user
/// information, which prevents links from disguising their destination.
#[must_use]
pub fn allowed_external_message_url(href: &str) -> Option<ExternalMessageLink> {
    let url = Url::parse(href).ok()?;
    let is_web_url = matches!(url.scheme(), "http" | "https");
    let has_no_user_info = url.username().is_empty() && url.password().is_none();
    (is_web_url && has_no_user_info).then(|| ExternalMessageLink(url.as_str().into()))
}

fn sanitize_html(raw_html: &str) -> Option<SanitizedEmailHtml> {
    let mut builder = Builder::default();
    builder
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .tag_attributes(HashMap::from([("a", HashSet::from(["href"]))]))
        .generic_attributes(HashSet::new())
        .url_schemes(HashSet::from(["http", "https"]))
        .url_relative(UrlRelative::Deny)
        .clean_content_tags(REMOVE_CONTENT_TAGS.iter().copied().collect());
    let sanitized = builder.clean(raw_html).to_string();
    (sanitized.len() <= MAX_HTML_BODY_BYTES).then(|| SanitizedEmailHtml(Arc::from(sanitized)))
}

#[cfg(test)]
mod tests {
    use einfach_mail_domain::{MessageBody, MessageBodyType};

    use super::{
        MAX_HTML_BODY_BYTES, MessageBodyForDisplay, allowed_external_message_url,
        prepare_message_body_for_display,
    };

    #[test]
    fn sanitizes_active_html_and_retains_common_formatting() {
        let body = MessageBody::new(
            MessageBodyType::Html,
            r#"<p onclick="run()"><strong>safe</strong><a href="https://mail.example.test">link</a><img src="https://tracker.example.test/pixel"></p><script>secret marker</script>"#.to_owned(),
        );

        let MessageBodyForDisplay::SanitizedHtml(html) = prepare_message_body_for_display(&body)
        else {
            panic!("normal-sized HTML is sanitized");
        };

        assert!(html.as_str().contains("<strong>safe</strong>"));
        assert!(html.as_str().contains("href=\"https://mail.example.test\""));
        assert!(!html.as_str().contains("onclick"));
        assert!(!html.as_str().contains("tracker.example.test"));
        assert!(!html.as_str().contains("secret marker"));
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
