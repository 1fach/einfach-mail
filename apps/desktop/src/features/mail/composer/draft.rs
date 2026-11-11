use einfach_mail_domain::{
    InvalidMailSubject, MailDraft, MailRecipient, MailReplyDraft, MailSubject,
};
use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::{AppContext, Context, Entity, Window};

use super::{ComposerTarget, MailComposer};

pub(super) struct ComposerFields {
    pub(super) recipient: Entity<InputState>,
    pub(super) subject: Entity<InputState>,
    pub(super) body: Entity<TextareaState>,
}

impl ComposerFields {
    pub(super) fn new(
        target: &ComposerTarget,
        window: &mut Window,
        cx: &mut Context<MailComposer>,
    ) -> Self {
        let recipient = cx.new(|cx| InputState::new(window, cx).placeholder("name@example.com"));
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Subject"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(match target {
                    ComposerTarget::NewMessage => "Write a plain-text message",
                    ComposerTarget::Reply { .. } => "Write a plain-text reply",
                })
                .rows(8)
        });
        Self {
            recipient,
            subject,
            body,
        }
    }
}

pub(super) enum ComposerSubmission {
    NewMessage(MailDraft),
    Reply(MailReplyDraft),
}

#[derive(Clone, Copy)]
pub(super) enum DraftError {
    InvalidRecipient,
    InvalidSubject(InvalidMailSubject),
}

impl MailComposer {
    pub(super) fn build_submission(
        &self,
        cx: &gpui_kit::App,
    ) -> Result<ComposerSubmission, DraftError> {
        let body = self.body.read(cx).value().to_string();
        match &self.target {
            ComposerTarget::NewMessage => self.build_new_message(cx, body),
            ComposerTarget::Reply { source, .. } => Ok(ComposerSubmission::Reply(
                MailReplyDraft::new(source.clone(), body),
            )),
        }
    }

    fn build_new_message(
        &self,
        cx: &gpui_kit::App,
        body: String,
    ) -> Result<ComposerSubmission, DraftError> {
        let recipient = self
            .recipient
            .read(cx)
            .value()
            .parse::<MailRecipient>()
            .map_err(|_| DraftError::InvalidRecipient)?;
        let subject = self
            .subject
            .read(cx)
            .value()
            .parse::<MailSubject>()
            .map_err(DraftError::InvalidSubject)?;
        Ok(ComposerSubmission::NewMessage(MailDraft::new(
            recipient, subject, body,
        )))
    }
}
