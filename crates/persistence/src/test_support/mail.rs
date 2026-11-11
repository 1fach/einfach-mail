use std::time::{Duration, SystemTime};

use super::provider::FixtureProvider;
use einfach_mail_application::{
    MailOperationError, MailSyncProgress, MessageDeltaPageResponse, MessageDeltaResponse,
};
use einfach_mail_domain::{
    AccountId, FolderId, FolderReference, MailDraft, MailFolder, MailRecipient, MailSubject,
    MessageBody, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress,
    MessageId, MessageMetadataPatch, ReadState,
};

pub(crate) fn folder(account_id: &AccountId) -> MailFolder {
    let reference = FolderReference::new(
        account_id.clone(),
        FolderId::try_from("folder-inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    MailFolder::new(reference, None, "Synthetic Inbox".to_owned())
        .expect("synthetic folder is valid")
}

pub(crate) fn archive_folder(account_id: &AccountId) -> MailFolder {
    let reference = FolderReference::new(
        account_id.clone(),
        FolderId::try_from("folder-archive".to_owned()).expect("synthetic folder ID is valid"),
    );
    MailFolder::new(reference, None, "Synthetic Archive".to_owned())
        .expect("synthetic archive folder is valid")
}

pub(crate) fn deleted_items_folder(account_id: &AccountId) -> MailFolder {
    let reference = FolderReference::new(
        account_id.clone(),
        FolderId::try_from("deleted-items".to_owned()).expect("synthetic folder ID is valid"),
    );
    MailFolder::new(reference, None, "Gelöschte Elemente".to_owned())
        .expect("synthetic Deleted Items folder is valid")
}

pub(crate) fn message_id() -> MessageId {
    MessageId::try_from("message-1".to_owned()).expect("synthetic message ID is valid")
}

pub(crate) fn draft(subject: &str, body: &str) -> MailDraft {
    MailDraft::new(
        "ada@example.test"
            .parse::<MailRecipient>()
            .expect("synthetic recipient is valid"),
        subject
            .parse::<MailSubject>()
            .expect("synthetic subject is valid"),
        body.to_owned(),
    )
}

pub(crate) fn first_page(cutoff: SystemTime, next_link: MessageDeltaLink) -> MessageDeltaPage {
    let mut recent = MessageMetadataPatch::new();
    recent.set_subject(Some("Synthetic subject".to_owned()));
    recent.set_from(None);
    recent.set_sender(None);
    recent.set_received_at(cutoff);
    recent.set_read_state(ReadState::Unread);
    let mut old = MessageMetadataPatch::new();
    old.set_subject(Some("Outside cutoff".to_owned()));
    old.set_from(None);
    old.set_sender(None);
    old.set_received_at(cutoff - Duration::from_nanos(1));
    old.set_read_state(ReadState::Read);
    MessageDeltaPage::new(
        vec![
            MessageDeltaChange::Upsert {
                id: message_id(),
                patch: recent,
            },
            MessageDeltaChange::Upsert {
                id: MessageId::try_from("old-message".to_owned())
                    .expect("synthetic message ID is valid"),
                patch: old,
            },
        ],
        MessageDeltaProgress::Next(next_link),
    )
}

pub(crate) fn final_page(delta_link: MessageDeltaLink) -> MessageDeltaPage {
    let mut patch = MessageMetadataPatch::new();
    patch.set_read_state(ReadState::Read);
    MessageDeltaPage::new(
        vec![MessageDeltaChange::Upsert {
            id: message_id(),
            patch,
        }],
        MessageDeltaProgress::Complete(delta_link),
    )
}

pub(crate) fn completed_message_delta_page(
    scope: &str,
    changes: Vec<MessageDeltaChange>,
) -> MessageDeltaPage {
    MessageDeltaPage::new(
        changes,
        MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(format!(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-{scope}"
        ))),
    )
}

pub(crate) fn page_response(page: MessageDeltaPage) -> MessageDeltaResponse {
    MessageDeltaResponse::Page(MessageDeltaPageResponse::new(page, 1_024))
}

pub(crate) fn provider_with_cached_message(
    account: AccountId,
    folders: Vec<MailFolder>,
    started_at: SystemTime,
    body: Result<MessageBody, MailOperationError>,
) -> FixtureProvider {
    let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
    let page = MessageDeltaPage::new(
        vec![MessageDeltaChange::Upsert {
            id: message_id(),
            patch: complete_patch("Synthetic subject".to_owned(), cutoff),
        }],
        MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-account-removal"
                .to_owned(),
        )),
    );
    FixtureProvider::new(account, folders, [Ok(page_response(page))], body)
}

pub(crate) fn ignore_sync_progress(_: MailSyncProgress) {}

pub(crate) fn complete_patch(subject: String, received_at: SystemTime) -> MessageMetadataPatch {
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(Some(subject));
    patch.set_from(None);
    patch.set_sender(None);
    patch.set_received_at(received_at);
    patch.set_read_state(ReadState::Unread);
    patch
}
