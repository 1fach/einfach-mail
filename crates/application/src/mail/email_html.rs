//! Sanitized message-body values prepared for native display.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex},
};

use ammonia::{Builder, UrlRelative};
use einfach_mail_domain::{MessageBody, MessageBodyType};
use url::Url;

const MAX_HTML_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_HTML_FALLBACK_INPUT_BYTES: usize = MAX_HTML_BODY_BYTES / 8;
// Ammonia can replace one-byte NUL characters with the eight-byte entity `&#65533;`.
const MAX_HTML_FALLBACK_TEXT_BYTES: usize = MAX_HTML_BODY_BYTES / 128;
const HTML_FALLBACK_WRAP_WIDTH: usize = 80;
const MAX_EXTERNAL_MESSAGE_LINKS: usize = 100;

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
/// Its private fields prevent callers from constructing this type from raw
/// provider or cache content.
#[derive(Clone, Eq, PartialEq)]
pub struct SanitizedEmailHtml {
    html: Arc<str>,
    external_links: Arc<[ExternalMessageLink]>,
    external_links_omitted: bool,
}

impl SanitizedEmailHtml {
    /// Returns the sanitized HTML fragment.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.html
    }

    /// Clones the immutable content without copying its bytes.
    #[must_use]
    pub fn shared_content(&self) -> Arc<str> {
        Arc::clone(&self.html)
    }

    /// Returns up to 100 distinct validated external links collected from the fragment.
    ///
    /// Check [`Self::external_links_omitted`] to learn whether the sanitizer found more.
    #[must_use]
    pub fn external_links(&self) -> &[ExternalMessageLink] {
        &self.external_links
    }

    /// Reports whether the sanitizer found more safe links than the exposed limit.
    #[must_use]
    pub fn external_links_omitted(&self) -> bool {
        self.external_links_omitted
    }
}

impl fmt::Debug for SanitizedEmailHtml {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SanitizedEmailHtml(<redacted>)")
    }
}

/// A validated HTTP or HTTPS destination from a message link.
#[derive(Clone, Eq, PartialEq)]
pub struct ExternalMessageLink {
    url: Arc<str>,
    host: Arc<str>,
}

impl ExternalMessageLink {
    /// Returns the normalized destination that passed message-link policy.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.url
    }

    /// Returns the normalized host without a path, query, or fragment.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
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
    let sanitized_fragment = sanitize_html(raw_html);
    if sanitized_fragment.html.len() <= MAX_HTML_BODY_BYTES {
        return MessageBodyForDisplay::SanitizedHtml(SanitizedEmailHtml {
            html: Arc::from(sanitized_fragment.html),
            external_links: sanitized_fragment.external_links,
            external_links_omitted: sanitized_fragment.external_links_omitted,
        });
    }

    plain_text_fallback(&sanitized_fragment.html).map_or(
        MessageBodyForDisplay::HtmlTooLarge,
        MessageBodyForDisplay::PlainTextFallback,
    )
}

/// Parses and validates a message link before the desktop opens it externally.
///
/// Returns a link only when it is absolute HTTP or HTTPS and contains no user
/// information before the host.
#[must_use]
pub fn allowed_external_message_url(href: &str) -> Option<ExternalMessageLink> {
    let url = Url::parse(href).ok()?;
    let is_web_url = matches!(url.scheme(), "http" | "https");
    let has_no_user_info = url.username().is_empty() && url.password().is_none();
    if !is_web_url || !has_no_user_info {
        return None;
    }
    let host = url.host_str()?;
    Some(ExternalMessageLink {
        url: Arc::from(url.as_str()),
        host: Arc::from(host),
    })
}

struct SanitizedFragment {
    html: String,
    external_links: Arc<[ExternalMessageLink]>,
    external_links_omitted: bool,
}

#[derive(Default)]
struct ExternalMessageLinkCollector {
    links: Vec<ExternalMessageLink>,
    seen: HashSet<Arc<str>>,
    omitted: bool,
}

impl ExternalMessageLinkCollector {
    fn push(&mut self, link: ExternalMessageLink) {
        if self.seen.contains(link.as_str()) {
            return;
        }
        if self.links.len() == MAX_EXTERNAL_MESSAGE_LINKS {
            self.omitted = true;
            return;
        }
        self.seen.insert(Arc::clone(&link.url));
        self.links.push(link);
    }
}

fn sanitize_html(raw_html: &str) -> SanitizedFragment {
    let collected_links = Arc::new(Mutex::new(ExternalMessageLinkCollector::default()));
    let sanitized_html = clean_html(raw_html, Arc::clone(&collected_links));
    let mut collected_links = collected_links
        .lock()
        .expect("sanitizer link collection remains unpoisoned");
    let external_links_omitted = collected_links.omitted;
    SanitizedFragment {
        html: sanitized_html,
        external_links: std::mem::take(&mut collected_links.links).into(),
        external_links_omitted,
    }
}

fn clean_html(raw_html: &str, collected_links: Arc<Mutex<ExternalMessageLinkCollector>>) -> String {
    let mut builder = Builder::default();
    builder
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .tag_attributes(HashMap::from([("a", HashSet::from(["href"]))]))
        .generic_attributes(HashSet::new())
        .url_schemes(HashSet::from(["http", "https"]))
        .url_relative(UrlRelative::Deny)
        .clean_content_tags(REMOVE_CONTENT_TAGS.iter().copied().collect())
        .attribute_filter(move |tag, attribute, value| {
            collect_external_message_link(&collected_links, tag, attribute, value);
            Some(value.into())
        });
    builder.clean(raw_html).to_string()
}

fn collect_external_message_link(
    collected_links: &Mutex<ExternalMessageLinkCollector>,
    tag: &str,
    attribute: &str,
    value: &str,
) {
    if tag != "a" || attribute != "href" {
        return;
    }
    let Some(link) = allowed_external_message_url(value) else {
        return;
    };
    collected_links
        .lock()
        .expect("sanitizer link collection remains unpoisoned")
        .push(link);
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
    Some(SanitizedEmailHtml {
        html: Arc::from(safe_html),
        external_links: Vec::new().into(),
        external_links_omitted: false,
    })
}

fn bounded_prefix(content: &str, max_bytes: usize) -> &str {
    let mut end = content.len().min(max_bytes);
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    &content[..end]
}

#[cfg(test)]
mod tests;
