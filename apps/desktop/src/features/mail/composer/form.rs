use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, Textarea},
    scroll::ScrollableElement,
};
use gpui_kit::{AnyElement, Context, IntoElement, Rems, Render, Window, div, prelude::*, rems};

use super::{ComposerStatus, ComposerTarget, MAIL_COMPOSER_WRITE_PENDING_CONTEXT, MailComposer};

const MAIL_COMPOSER_BODY_HEIGHT: Rems = rems(13.75);

#[derive(Clone, Copy)]
enum FieldAvailability {
    Editable,
    Disabled,
}

impl FieldAvailability {
    fn is_disabled(self) -> bool {
        matches!(self, Self::Disabled)
    }
}

impl MailComposer {
    pub(super) fn can_send(&self) -> bool {
        self.pending_send.pending().is_none()
            && self.pending_send.is_available()
            && !matches!(
                self.status,
                ComposerStatus::Sending
                    | ComposerStatus::RenewingPermission
                    | ComposerStatus::Accepted
                    | ComposerStatus::DuplicateConfirmation
            )
    }

    fn render_field(
        id: &'static str,
        label: &'static str,
        input: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().font_bold().child(label))
            .child(input)
    }

    fn field_availability(&self) -> FieldAvailability {
        if self.pending_send.pending().is_some()
            || !self.pending_send.is_available()
            || self.task.is_some()
            || matches!(
                self.status,
                ComposerStatus::Accepted | ComposerStatus::DuplicateConfirmation
            )
        {
            FieldAvailability::Disabled
        } else {
            FieldAvailability::Editable
        }
    }

    fn render_composer_content(
        &mut self,
        field_availability: FieldAvailability,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id("mail-composer")
            .debug_selector(|| "mail-composer".to_owned())
            .size_full()
            .when(self.operation_in_progress(), |element| {
                element.key_context(MAIL_COMPOSER_WRITE_PENDING_CONTEXT)
            })
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .overflow_y_scrollbar()
            .children(self.render_composer_children(&colors, field_availability, cx))
    }

    fn render_composer_children(
        &mut self,
        colors: &ColorTokens,
        field_availability: FieldAvailability,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut children = vec![self.render_pending(colors, cx)];
        children.extend(self.render_target_fields(field_availability));
        children.push(self.render_body_field(field_availability));
        children.extend(self.render_status(colors, cx));
        children.push(self.render_disclaimer(colors));
        children.push(self.render_send_action(cx));
        children
    }

    fn render_target_fields(&self, field_availability: FieldAvailability) -> Vec<AnyElement> {
        match &self.target {
            ComposerTarget::NewMessage => vec![
                self.render_recipient_field(field_availability),
                self.render_subject_field(field_availability),
            ],
            ComposerTarget::Reply { subject, .. } => {
                vec![Self::render_reply_context(subject.as_deref())]
            }
        }
    }

    fn render_recipient_field(&self, field_availability: FieldAvailability) -> AnyElement {
        Self::render_field(
            "mail-recipient-field",
            "To",
            div()
                .id("mail-recipient-input")
                .debug_selector(|| "mail-recipient-input".to_owned())
                .child(
                    Input::new(&self.recipient)
                        .aria_label("Recipient")
                        .disabled(field_availability.is_disabled())
                        .w_full(),
                ),
        )
        .into_any_element()
    }

    fn render_subject_field(&self, field_availability: FieldAvailability) -> AnyElement {
        Self::render_field(
            "mail-subject-field",
            "Subject",
            div()
                .id("mail-subject-input")
                .debug_selector(|| "mail-subject-input".to_owned())
                .child(
                    Input::new(&self.subject)
                        .aria_label("Subject")
                        .disabled(field_availability.is_disabled())
                        .w_full(),
                ),
        )
        .into_any_element()
    }

    fn render_reply_context(subject: Option<&str>) -> AnyElement {
        div()
            .id("mail-reply-context")
            .debug_selector(|| "mail-reply-context".to_owned())
            .text_sm()
            .child("Microsoft will use the original message's reply recipients, including Reply-To when present.")
            .children(subject.map(|subject| {
                div().text_sm().child(format!("Original subject: {subject}"))
            }))
            .into_any_element()
    }

    fn render_body_field(&self, field_availability: FieldAvailability) -> AnyElement {
        let (label, aria_label) = self.body_labels();
        Self::render_field(
            "mail-body-field",
            label,
            div()
                .id("mail-body-input")
                .debug_selector(|| "mail-body-input".to_owned())
                .child(
                    Textarea::new(&self.body)
                        .aria_label(aria_label)
                        .disabled(field_availability.is_disabled())
                        .h(MAIL_COMPOSER_BODY_HEIGHT)
                        .w_full(),
                ),
        )
        .into_any_element()
    }

    fn body_labels(&self) -> (&'static str, &'static str) {
        match &self.target {
            ComposerTarget::NewMessage => ("Message", "Plain-text message"),
            ComposerTarget::Reply { .. } => ("Reply", "Plain-text reply"),
        }
    }

    fn render_disclaimer(&self, colors: &ColorTokens) -> AnyElement {
        let text = match &self.target {
            ComposerTarget::NewMessage => {
                "Messages use plain text and one recipient. The app saves no message body in its local send status."
            }
            ComposerTarget::Reply { .. } => {
                "Replies use plain text. The app keeps reply content in memory and saves no body in its local send status."
            }
        };
        div()
            .text_xs()
            .text_color(colors.muted_foreground)
            .child(text)
            .into_any_element()
    }

    fn render_send_action(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                Button::new("mail-send")
                    .primary()
                    .debug_selector(|| "mail-send-button".to_owned())
                    .label(if matches!(self.status, ComposerStatus::Sending) {
                        "Sending…"
                    } else {
                        "Send"
                    })
                    .loading(matches!(self.status, ComposerStatus::Sending))
                    .disabled(!self.can_send())
                    .on_click(cx.listener(|composer, _, _, cx| composer.submit(cx))),
            )
            .into_any_element()
    }
}

impl Render for MailComposer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field_availability = self.field_availability();
        self.render_composer_content(field_availability, cx)
    }
}
