use einfach_mail_domain::{
    AccountCapabilities, AccountId, AccountProfile, CalendarAccess, MailCapabilities, MailMessage,
    MessageId,
};

use super::super::synthetic_account_profile;
use super::cache::FixtureCache;

pub(crate) const SYNTHETIC_MESSAGE_LABEL_PREFIX: &str =
    "Ada Example <ada@example.test>. Synthetic subject";

pub(crate) fn mail_profile_with_capabilities(
    account: AccountId,
    mail: MailCapabilities,
) -> AccountProfile {
    synthetic_account_profile(
        account,
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::new(mail, CalendarAccess::ReadOnly),
    )
    .expect("synthetic account profile has an identity")
}

pub(crate) fn append_synthetic_messages(
    cache: &FixtureCache,
    message_count: usize,
) -> Vec<MessageId> {
    let mut state = cache.0.lock().expect("fixture cache mutex is healthy");
    let template = state
        .messages
        .first()
        .cloned()
        .expect("fixture cache has a synthetic message");
    let mut message_ids = vec![template.id().clone()];
    for index in 2..=message_count {
        let message_id =
            MessageId::try_from(format!("message-{index}")).expect("synthetic message ID is valid");
        let template_metadata = template.metadata();
        let metadata = einfach_mail_domain::MessageMetadata::new(
            Some(format!("Synthetic subject {index}")),
            template_metadata.origin().clone(),
            *template_metadata.received_at(),
            template_metadata.read_state(),
            template_metadata.follow_up_flag_status(),
        );
        state.messages.push(MailMessage::new(
            template.folder().clone(),
            message_id.clone(),
            metadata,
        ));
        message_ids.push(message_id);
    }
    message_ids
}
