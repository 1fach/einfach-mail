use einfach_mail_domain::{MessageBody, MessageBodyType};

use super::{
    MAX_EXTERNAL_MESSAGE_LINKS, MAX_HTML_BODY_BYTES, MAX_HTML_FALLBACK_TEXT_BYTES,
    MessageBodyForDisplay, allowed_external_message_url, prepare_message_body_for_display,
};

fn sanitized_html_for_test(raw_html: &str) -> String {
    let body = MessageBody::new(MessageBodyType::Html, raw_html.to_owned());
    let MessageBodyForDisplay::SanitizedHtml(html) = prepare_message_body_for_display(&body) else {
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

    let MessageBodyForDisplay::SanitizedHtml(html) = prepare_message_body_for_display(&body) else {
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
fn collects_only_distinct_safe_links_from_retained_content() {
    let body = MessageBody::new(
        MessageBodyType::Html,
        concat!(
            "<p><a href=\"https://mail.example.test/thread?tracking=marker\">first</a></p>",
            "<p><a href=\"https://mail.example.test/thread?tracking=marker\">duplicate</a></p>",
            "<a href=\"https://person@mail.example.test/private\">userinfo</a>",
            "<svg><a href=\"https://hidden.example.test/removed\">hidden svg link</a></svg>",
            "<script><a href=\"https://hidden.example.test/script\">hidden script link</a></script>",
            "<a href=\"javascript:alert(1)\">unsafe</a>"
        )
        .to_owned(),
    );

    let MessageBodyForDisplay::SanitizedHtml(html) = prepare_message_body_for_display(&body) else {
        panic!("normal-sized HTML is sanitized");
    };
    let links = html.external_links();

    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].as_str(),
        "https://mail.example.test/thread?tracking=marker"
    );
    assert_eq!(links[0].host(), "mail.example.test");
    assert!(!html.external_links_omitted());
    assert!(
        !format!("{links:?}").contains("tracking=marker"),
        "link debug output must redact its destination"
    );
}

#[test]
fn reports_when_the_distinct_safe_link_limit_is_exceeded() {
    let build_body = |link_count: usize| {
        let links = (0..link_count)
            .map(|index| {
                format!(
                    "<a href=\"https://link-{index}.example.test/private?tracking=marker-{index}\">link</a>"
                )
            })
            .collect::<String>();
        MessageBody::new(MessageBodyType::Html, links)
    };

    let within_limit = prepare_message_body_for_display(&build_body(MAX_EXTERNAL_MESSAGE_LINKS));
    let MessageBodyForDisplay::SanitizedHtml(within_limit) = within_limit else {
        panic!("normal-sized HTML remains sanitized");
    };
    assert_eq!(
        within_limit.external_links().len(),
        MAX_EXTERNAL_MESSAGE_LINKS
    );
    assert!(!within_limit.external_links_omitted());

    let over_limit = prepare_message_body_for_display(&build_body(MAX_EXTERNAL_MESSAGE_LINKS + 1));
    let MessageBodyForDisplay::SanitizedHtml(over_limit) = over_limit else {
        panic!("normal-sized HTML remains sanitized");
    };
    assert_eq!(
        over_limit.external_links().len(),
        MAX_EXTERNAL_MESSAGE_LINKS
    );
    assert!(over_limit.external_links_omitted());
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

    let MessageBodyForDisplay::SanitizedHtml(html) = prepare_message_body_for_display(&body) else {
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
        "https://:password@mail.example.test/path",
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
