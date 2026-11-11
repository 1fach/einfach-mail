use std::time::{Duration, SystemTime};

use crate::{AccountId, FolderId, FolderReference, Mailbox};

use super::{
    FolderParentError, FollowUpFlagStatus, MailFolder, MessageBody, MessageBodyType,
    MessageMetadata, MessageMetadataPatch, MessageOrigin, ReadState,
};

fn folder_reference(account_id: &str, folder_id: &str) -> FolderReference {
    let account_id =
        AccountId::try_from(account_id.to_owned()).expect("synthetic account ID is non-empty");
    let folder_id =
        FolderId::try_from(folder_id.to_owned()).expect("synthetic folder ID is non-empty");
    FolderReference::new(account_id, folder_id)
}

#[test]
fn mail_folder_rejects_parent_from_another_account() {
    let reference = folder_reference("account-1", "folder-1");
    let parent = folder_reference("account-2", "folder-2");

    assert_eq!(
        MailFolder::new(reference, Some(parent), "Child".to_owned()),
        Err(FolderParentError::AccountMismatch)
    );
}

#[test]
fn mail_folder_rejects_itself_as_parent() {
    let reference = folder_reference("account-1", "folder-1");
    let parent = reference.clone();

    assert_eq!(
        MailFolder::new(reference, Some(parent), "Folder".to_owned()),
        Err(FolderParentError::SelfParent)
    );
}

#[test]
fn mail_folder_accepts_a_distinct_parent_from_the_same_account() {
    let reference = folder_reference("account-1", "folder-1");
    let parent = folder_reference("account-1", "folder-2");

    assert!(MailFolder::new(reference, Some(parent), "Child".to_owned()).is_ok());
}

#[test]
fn message_metadata_patch_preserves_unchanged_fields() {
    let original = synthetic_metadata();
    let mut metadata = original.clone();

    metadata.apply_patch(MessageMetadataPatch::new());

    assert_eq!(metadata, original);
}

#[test]
fn message_metadata_patch_clears_nullable_and_replaces_present_fields() {
    let replacement_time = SystemTime::UNIX_EPOCH + Duration::from_secs(2);
    let replacement_sender = synthetic_mailbox("new-sender@example.test");
    let mut metadata = synthetic_metadata();
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(None);
    patch.set_from(None);
    patch.set_sender(Some(replacement_sender.clone()));
    patch.set_received_at(replacement_time);
    patch.set_read_state(ReadState::Unread);
    patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::Complete));

    metadata.apply_patch(patch);

    assert_eq!(metadata.subject(), None);
    assert_eq!(metadata.origin().from_mailbox(), None);
    assert_eq!(metadata.origin().sender(), Some(&replacement_sender));
    assert_eq!(metadata.received_at(), &replacement_time);
    assert_eq!(metadata.read_state(), ReadState::Unread);
    assert_eq!(
        metadata.follow_up_flag_status(),
        Some(FollowUpFlagStatus::Complete)
    );
}

#[test]
fn complete_delta_patch_creates_message_with_explicit_nullable_fields() {
    let folder = folder_reference("account-1", "folder-1");
    let id = crate::MessageId::try_from("message-1".to_owned())
        .expect("synthetic message ID is non-empty");
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(None);
    patch.set_from(None);
    patch.set_sender(Some(synthetic_mailbox("sender@example.test")));
    patch.set_received_at(SystemTime::UNIX_EPOCH);
    patch.set_read_state(ReadState::Read);

    let message = patch
        .into_complete_message(folder.clone(), id)
        .expect("all selected fields are present");

    assert_eq!(message.folder(), &folder);
    assert_eq!(message.metadata().subject(), None);
    assert!(message.metadata().origin().from_mailbox().is_none());
    assert_eq!(
        message.metadata().origin().sender().map(Mailbox::address),
        Some("sender@example.test")
    );
}

#[test]
fn incomplete_delta_patch_cannot_create_a_message() {
    let folder = folder_reference("account-1", "folder-1");
    let id = crate::MessageId::try_from("message-1".to_owned())
        .expect("synthetic message ID is non-empty");
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(Some("Synthetic subject".to_owned()));

    assert!(patch.into_complete_message(folder, id).is_none());
}

#[test]
fn message_body_debug_output_redacts_content() {
    let body = MessageBody::new(MessageBodyType::Html, "private synthetic body".to_owned());

    let debug_output = format!("{body:?}");

    assert!(debug_output.contains("<redacted>"));
    assert!(!debug_output.contains("private synthetic body"));
}

#[test]
fn mail_debug_output_redacts_nested_account_and_message_values() {
    let folder_reference = folder_reference("account-id-marker", "folder-id-marker");
    let folder = MailFolder::new(
        folder_reference.clone(),
        None,
        "folder-name-marker".to_owned(),
    )
    .expect("synthetic folder is valid");
    let sender = Mailbox::try_new(
        "sender-address-marker@example.test".to_owned(),
        Some("sender-name-marker".to_owned()),
    )
    .expect("synthetic sender mailbox is valid");
    let metadata = MessageMetadata::new(
        Some("message-subject-marker".to_owned()),
        MessageOrigin::new(Some(sender.clone()), Some(sender.clone())),
        SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000),
        ReadState::Read,
        Some(FollowUpFlagStatus::Flagged),
    );
    let message = super::MailMessage::new(
        folder_reference.clone(),
        "message-id-marker"
            .to_owned()
            .try_into()
            .expect("synthetic message ID is non-empty"),
        metadata,
    );
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(Some("patch-subject-marker".to_owned()));
    patch.set_from(Some(sender));
    patch.set_received_at(SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000));
    let page = super::MessageDeltaPage::new(
        vec![super::MessageDeltaChange::Upsert {
            id: "delta-message-id-marker"
                .to_owned()
                .try_into()
                .expect("synthetic message ID is non-empty"),
            patch: patch.clone(),
        }],
        super::MessageDeltaProgress::Next(super::MessageDeltaLink::from_opaque(
            "https://graph.example.test/?cursor=cursor-marker".to_owned(),
        )),
    );
    let debug = format!("{folder:?}{message:?}{patch:?}{page:?}");

    for marker in [
        "account-id-marker",
        "folder-id-marker",
        "message-id-marker",
        "delta-message-id-marker",
        "folder-name-marker",
        "sender-address-marker",
        "sender-name-marker",
        "message-subject-marker",
        "patch-subject-marker",
        "cursor-marker",
        "2000000000",
    ] {
        assert!(!debug.contains(marker), "Debug output included {marker:?}");
    }
    assert!(debug.contains("Read"));
    assert!(debug.contains("Flagged"));
    assert!(debug.contains("<redacted>"));
}

fn synthetic_metadata() -> MessageMetadata {
    MessageMetadata::new(
        Some("Synthetic subject".to_owned()),
        MessageOrigin::new(
            Some(synthetic_mailbox("from@example.test")),
            Some(synthetic_mailbox("sender@example.test")),
        ),
        SystemTime::UNIX_EPOCH,
        ReadState::Read,
        None,
    )
}

fn synthetic_mailbox(address: &str) -> Mailbox {
    Mailbox::try_new(address.to_owned(), None).expect("synthetic mailbox address is valid")
}
