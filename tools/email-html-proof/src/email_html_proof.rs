#[cfg(target_os = "linux")]
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use einfach_mail_application::{
    MessageBodyForDisplay, allowed_external_message_url, prepare_message_body_for_display,
};
use einfach_mail_domain::{MessageBody, MessageBodyType};

#[cfg(target_os = "linux")]
use gpui_kit::{
    Context, ImageSource, InteractiveElement, IntoElement, Modifiers, MouseButton, ParentElement,
    Render, Styled, TestAppContext, VisualTestContext, Window,
    base::{TextSelection, TextSelectionLayer},
    component::text::TextView,
    div, point, px,
};

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
    <a href="http://mail.example.invalid/legacy">legacy link</a>
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

const UNAPPROVED_ATTRIBUTE_POLICY_FIXTURE: &str = r#"
    <p class="mail" id="content" lang="en" title="Paragraph"
       style="color:red" onclick="event marker" onmouseover="hover marker">Text</p>
    <a href="https://mail.example.invalid" target="_blank" onfocus="focus marker">Link</a>
    <link rel="stylesheet" href="https://tracker.invalid/style.css">
    <meta http-equiv="refresh" content="0;url=https://tracker.invalid/refresh">
    <base href="https://tracker.invalid/">
"#;

const UNAPPROVED_ATTRIBUTE_FORBIDDEN_FRAGMENTS: &[&str] = &[
    "class=",
    "id=",
    "lang=",
    "title=",
    "style=",
    "onclick",
    "onmouseover",
    "onfocus",
    "target=",
    "event marker",
    "hover marker",
    "focus marker",
    "tracker.invalid",
    "<link",
    "<meta",
    "<base",
];

fn sanitize(raw_html: &str) -> String {
    let body = MessageBody::new(MessageBodyType::Html, raw_html.to_owned());
    match prepare_message_body_for_display(&body) {
        MessageBodyForDisplay::SanitizedHtml(html) => html.as_str().to_owned(),
        MessageBodyForDisplay::PlainTextFallback(_) => {
            panic!("synthetic fixture remains within the rich-render limit")
        }
        MessageBodyForDisplay::HtmlTooLarge => panic!("synthetic fixture is within the body limit"),
        MessageBodyForDisplay::PlainText(_) => panic!("HTML fixture remains HTML"),
    }
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
    assert!(sanitized.contains("href=\"http://mail.example.invalid/legacy\""));
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
fn strips_unapproved_attributes_and_document_navigation_tags() {
    let sanitized = sanitize(UNAPPROVED_ATTRIBUTE_POLICY_FIXTURE);

    assert!(sanitized.contains("Text"));
    assert!(sanitized.contains("Link"));
    assert_absent(&sanitized, UNAPPROVED_ATTRIBUTE_FORBIDDEN_FRAGMENTS);
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

#[cfg(target_os = "linux")]
struct EmailBodyTestView {
    html: String,
    link_clicks: Arc<Mutex<Vec<String>>>,
    image_resolutions: Arc<AtomicUsize>,
}

#[cfg(target_os = "linux")]
#[test]
fn only_http_and_https_urls_are_external_link_targets() {
    for href in [
        "http://mail.example.invalid",
        "https://mail.example.invalid",
    ] {
        assert!(
            allowed_external_message_url(href).is_some(),
            "web URL rejected: {href}"
        );
    }

    for href in [
        "javascript:alert(1)",
        "data:text/html,unsafe",
        "mailto:person@example.invalid",
        "file:///etc/passwd",
        "/relative/path",
        "//tracker.invalid/path",
    ] {
        assert!(
            allowed_external_message_url(href).is_none(),
            "non-web URL accepted: {href}"
        );
    }
}

#[cfg(target_os = "linux")]
fn mount_email_body(
    test_app: &mut TestAppContext,
    html: String,
    link_clicks: Arc<Mutex<Vec<String>>>,
    image_resolutions: Arc<AtomicUsize>,
) -> &mut VisualTestContext {
    test_app.update(gpui_kit::init);
    let (_, visual_context) = test_app.add_window_view(move |_, _| EmailBodyTestView {
        html,
        link_clicks,
        image_resolutions,
    });
    visual_context.run_until_parked();
    visual_context.update(|window, app| {
        let _ = window.draw(app);
    });
    visual_context
}

#[cfg(target_os = "linux")]
impl Render for EmailBodyTestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let link_clicks = Arc::clone(&self.link_clicks);
        let image_resolutions = Arc::clone(&self.image_resolutions);

        div()
            .w(px(600.))
            .debug_selector(|| "email-body".into())
            .child(TextSelectionLayer)
            .child(
                TextView::html("email-body-content", self.html.clone())
                    .selectable(true)
                    .image_source(move |_| {
                        image_resolutions.fetch_add(1, Ordering::SeqCst);
                        ImageSource::Custom(Arc::new(|_, _| None))
                    })
                    .on_link_click(move |url, event, _, cx| {
                        if event.is_secondary() {
                            return;
                        }
                        let Some(parsed_url) = allowed_external_message_url(url.as_ref()) else {
                            return;
                        };
                        link_clicks
                            .lock()
                            .expect("test link recorder lock is healthy")
                            .push(url.to_string());
                        cx.open_url(parsed_url.as_str());
                    }),
            )
    }
}

#[cfg(target_os = "linux")]
#[gpui::test]
fn native_html_renderer_routes_safe_links_only_after_a_click_and_does_not_resolve_images(
    cx: &mut TestAppContext,
) {
    let html = sanitize(
        r#"
            <p><a href="https://mail.example.invalid/thread">Open thread</a></p>
            <img src="https://tracker.invalid/pixel.png" alt="remote image">
            <script>untrusted script marker</script>
        "#,
    );
    let link_clicks = Arc::new(Mutex::new(Vec::new()));
    let image_resolutions = Arc::new(AtomicUsize::new(0));
    let cx = mount_email_body(
        cx,
        html,
        Arc::clone(&link_clicks),
        Arc::clone(&image_resolutions),
    );

    assert_eq!(cx.opened_url(), None);
    assert_eq!(image_resolutions.load(Ordering::SeqCst), 0);

    let position = point(px(10.), px(10.));
    cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    assert_eq!(cx.opened_url(), None);
    assert!(
        link_clicks
            .lock()
            .expect("test link recorder lock is healthy")
            .is_empty()
    );

    cx.simulate_click(point(px(10.), px(10.)), Modifiers::default());

    assert_eq!(
        link_clicks
            .lock()
            .expect("test link recorder lock is healthy")
            .as_slice(),
        ["https://mail.example.invalid/thread"]
    );
    assert_eq!(
        cx.opened_url(),
        Some("https://mail.example.invalid/thread".to_owned())
    );
    assert_eq!(image_resolutions.load(Ordering::SeqCst), 0);
}

#[cfg(target_os = "linux")]
#[gpui::test]
fn native_html_renderer_does_not_open_an_unsafe_sanitized_link(cx: &mut TestAppContext) {
    let html = sanitize(r#"<p><a href="javascript:alert(1)">unsafe destination</a></p>"#);
    assert!(!html.contains("href="));

    let link_clicks = Arc::new(Mutex::new(Vec::new()));
    let image_resolutions = Arc::new(AtomicUsize::new(0));
    let cx = mount_email_body(
        cx,
        html,
        Arc::clone(&link_clicks),
        Arc::clone(&image_resolutions),
    );

    cx.simulate_click(point(px(10.), px(10.)), Modifiers::default());

    assert!(
        link_clicks
            .lock()
            .expect("test link recorder lock is healthy")
            .is_empty()
    );
    assert_eq!(cx.opened_url(), None);
}

#[cfg(target_os = "linux")]
#[gpui::test]
fn native_html_renderer_supports_selection_and_copy(cx: &mut TestAppContext) {
    let cx = mount_email_body(
        cx,
        "Readable message content for selection and copy".to_owned(),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(AtomicUsize::new(0)),
    );

    let body_bounds = cx.debug_bounds("email-body").expect("email body bounds");
    let start = point(body_bounds.left() + px(1.), body_bounds.top() + px(8.));
    let end = point(body_bounds.right() - px(1.), body_bounds.top() + px(8.));
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.update(|window, app| {
        let _ = window.draw(app);
    });
    cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    cx.update(|window, app| {
        let _ = window.draw(app);
    });
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    cx.update(|window, app| {
        let _ = window.draw(app);
    });

    let selected_text = cx.update(TextSelection::selected_text);
    assert!(
        selected_text.contains("Readable message content"),
        "unexpected selected text: {selected_text:?}"
    );

    cx.simulate_keystrokes("ctrl-c");
    let copied_text = cx.update(|_, app| {
        app.read_from_clipboard()
            .and_then(|clipboard| clipboard.text())
    });
    assert_eq!(copied_text, Some(selected_text.trim_end().to_owned()));
}
