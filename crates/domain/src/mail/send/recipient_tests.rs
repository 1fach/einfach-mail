use super::{
    MailDraft, MailRecipient, MailReplyDraft, MailReplySource, MailSendOperationId, MailSubject,
};
use crate::{AccountId, FolderId, FolderReference, MessageId};

#[test]
fn parses_a_single_bare_address_and_trims_outer_whitespace() {
    let recipient: MailRecipient = "  ada+project@example.test  "
        .parse()
        .expect("synthetic recipient is a valid bare address");

    assert_eq!(recipient.as_str(), "ada+project@example.test");
}

#[test]
fn rejects_display_names_and_multiple_recipients() {
    assert!(
        "Ada Example <ada@example.test>"
            .parse::<MailRecipient>()
            .is_err()
    );
    assert!(
        "ada@example.test, grace@example.test"
            .parse::<MailRecipient>()
            .is_err()
    );
    assert!("not an address".parse::<MailRecipient>().is_err());
}

#[test]
fn rejects_empty_and_multiline_subjects() {
    assert_eq!(
        " \t ".parse::<MailSubject>(),
        Err(super::InvalidMailSubject::Empty)
    );
    assert_eq!(
        "Synthetic\nsubject".parse::<MailSubject>(),
        Err(super::InvalidMailSubject::Multiline)
    );
}

#[test]
fn message_debug_output_redacts_recipient_subject_and_body() {
    let recipient = "ada@example.test"
        .parse::<MailRecipient>()
        .expect("synthetic recipient is a valid bare address");
    let subject = "Synthetic subject"
        .parse::<MailSubject>()
        .expect("synthetic subject is a single line");
    let draft = MailDraft::new(recipient, subject, "Synthetic body".to_owned());
    let pending = draft.pending_send(
        AccountId::try_from("pending-account-id-marker".to_owned())
            .expect("synthetic account ID is non-empty"),
        MailSendOperationId::new(),
        std::time::UNIX_EPOCH,
    );

    let draft_debug = format!("{draft:?}");
    let pending_debug = format!("{pending:?}");
    for value in [
        "ada@example.test",
        "Synthetic subject",
        "Synthetic body",
        "pending-account-id-marker",
    ] {
        assert!(!draft_debug.contains(value));
        assert!(!pending_debug.contains(value));
    }
}

#[test]
fn reply_debug_output_redacts_source_and_body() {
    let account = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    let folder = FolderReference::new(
        account,
        FolderId::try_from("private-folder-id".to_owned())
            .expect("synthetic folder ID is non-empty"),
    );
    let source = MailReplySource::new(
        folder,
        MessageId::try_from("private-message-id".to_owned())
            .expect("synthetic message ID is non-empty"),
    );
    let draft = MailReplyDraft::new(source, "Synthetic private reply".to_owned());
    let pending = draft.pending_send(
        MailSendOperationId::new(),
        Some("Synthetic source subject"),
        std::time::UNIX_EPOCH,
    );
    let draft_debug = format!("{draft:?}");
    let pending_debug = format!("{pending:?}");

    for value in [
        "private-folder-id",
        "private-message-id",
        "Synthetic private reply",
        "Synthetic source subject",
    ] {
        assert!(!draft_debug.contains(value));
        assert!(!pending_debug.contains(value));
    }
}
