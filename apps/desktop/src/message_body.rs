use std::sync::Arc;

use einfach_mail_application::{
    MessageBodyForDisplay, SanitizedEmailHtml, allowed_external_message_url,
};
use gpui_kit::component::ColorTokens;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, ImageSource, IntoElement, Role, SharedString, base::TextSelectionLayer, div,
};

pub(super) fn render_message_body(body: &MessageBodyForDisplay, colors: ColorTokens) -> AnyElement {
    match body {
        MessageBodyForDisplay::PlainText(text) => render_plain_text_body(text),
        MessageBodyForDisplay::SanitizedHtml(html) => render_sanitized_html_body(html),
        MessageBodyForDisplay::PlainTextFallback(content) => {
            render_plain_text_fallback(content, colors)
        }
        MessageBodyForDisplay::HtmlTooLarge => render_oversized_html_fallback(colors),
    }
}

fn render_plain_text_body(text: &Arc<str>) -> AnyElement {
    div()
        .id("mail-message-plain-body")
        .flex_1()
        .min_h_0()
        .overflow_y_scrollbar()
        .p_6()
        .child(SharedString::from(Arc::clone(text)))
        .into_any_element()
}

fn render_plain_text_fallback(content: &SanitizedEmailHtml, colors: ColorTokens) -> AnyElement {
    div()
        .id("mail-message-html-fallback")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .p_6()
        .child(render_plain_text_fallback_status(colors))
        .child(render_plain_text_fallback_content(content))
        .into_any_element()
}

fn render_plain_text_fallback_status(colors: ColorTokens) -> AnyElement {
    div()
        .id("mail-message-html-fallback-status")
        .debug_selector(|| "mail-message-html-fallback-status".into())
        .text_color(colors.muted_foreground)
        .role(Role::Status)
        .child(
            "Rich formatting was removed. Only the beginning of this message is shown as plain text.",
        )
        .into_any_element()
}

fn render_plain_text_fallback_content(content: &SanitizedEmailHtml) -> AnyElement {
    div()
        .id("mail-message-html-fallback-body")
        .debug_selector(|| "mail-message-html-fallback-body".into())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scrollbar()
        .child(TextSelectionLayer)
        .child(
            TextView::html(
                "mail-message-html-fallback-content",
                content.shared_content(),
            )
            .selectable(true)
            .image_source(|_| ImageSource::Custom(Arc::new(|_, _| None)))
            .on_link_click(|_, _, _, _| {}),
        )
        .into_any_element()
}

fn render_sanitized_html_body(html: &SanitizedEmailHtml) -> AnyElement {
    div()
        .id("mail-message-html-body")
        .debug_selector(|| "mail-message-html-body".into())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scrollbar()
        .p_6()
        .child(TextSelectionLayer)
        .child(
            TextView::html("mail-message-html-content", html.shared_content())
                .selectable(true)
                .image_source(|_| ImageSource::Custom(Arc::new(|_, _| None)))
                .on_link_click(|href, event, _, cx| {
                    if event.is_secondary() {
                        return;
                    }
                    if let Some(url) = allowed_external_message_url(href.as_ref()) {
                        cx.open_url(url.as_str());
                    }
                }),
        )
        .into_any_element()
}

fn render_oversized_html_fallback(colors: ColorTokens) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .p_6()
        .text_color(colors.muted_foreground)
        .id("mail-message-body-too-large")
        .debug_selector(|| "mail-message-body-too-large".into())
        .role(Role::Status)
        .child("This message is too large to display safely.")
        .into_any_element()
}
