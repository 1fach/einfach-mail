use std::time::{Duration, SystemTime};

use crate::{AccountId, FolderId, FolderReference, Mailbox};

use super::{
    FolderParentError, FollowUpFlagStatus, MailFolder, MailFolderKind, MessageBody,
    MessageBodyType, MessageMetadata, MessageMetadataPatch, MessageOrigin, MessagePreview,
    ReadState, UnreadCount,
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

fn synthetic_preview(text: &str) -> MessagePreview {
    MessagePreview::from_untrusted(text).expect("synthetic preview has printable text")
}

#[test]
fn mail_folder_starts_without_a_kind_or_unread_count() {
    let folder = MailFolder::new(
        folder_reference("account-1", "folder-1"),
        None,
        "Folder".to_owned(),
    )
    .expect("synthetic folder is valid");

    assert_eq!(folder.kind(), None);
    assert_eq!(folder.unread_count(), None);
}

#[test]
fn mail_folder_builders_set_and_clear_the_kind_and_unread_count() {
    let folder = MailFolder::new(
        folder_reference("account-1", "folder-1"),
        None,
        "Folder".to_owned(),
    )
    .expect("synthetic folder is valid")
    .with_kind(Some(MailFolderKind::Inbox))
    .with_unread_count(Some(UnreadCount::new(12)));

    assert_eq!(folder.kind(), Some(MailFolderKind::Inbox));
    assert_eq!(folder.unread_count().map(UnreadCount::get), Some(12));

    let cleared = folder.with_kind(None).with_unread_count(None);

    assert_eq!(cleared.kind(), None);
    assert_eq!(cleared.unread_count(), None);
}

#[test]
fn message_metadata_starts_without_a_preview() {
    assert_eq!(synthetic_metadata().preview(), None);
}

#[test]
fn message_metadata_with_preview_sets_and_clears_the_preview() {
    let preview = synthetic_preview("Synthetic opening line");

    let metadata = synthetic_metadata().with_preview(Some(preview.clone()));

    assert_eq!(metadata.preview(), Some(&preview));
    assert_eq!(metadata.with_preview(None).preview(), None);
}

#[test]
fn unchanged_preview_patch_keeps_the_preview() {
    let preview = synthetic_preview("Synthetic opening line");
    let mut metadata = synthetic_metadata().with_preview(Some(preview.clone()));

    metadata.apply_patch(MessageMetadataPatch::new());

    assert_eq!(metadata.preview(), Some(&preview));
}

#[test]
fn preview_patch_set_none_clears_the_preview() {
    let mut metadata = synthetic_metadata().with_preview(Some(synthetic_preview("Synthetic")));
    let mut patch = MessageMetadataPatch::new();
    patch.set_preview(None);

    metadata.apply_patch(patch);

    assert_eq!(metadata.preview(), None);
}

#[test]
fn preview_patch_set_some_replaces_the_preview() {
    let replacement = synthetic_preview("Replacement opening line");
    let mut metadata = synthetic_metadata().with_preview(Some(synthetic_preview("Synthetic")));
    let mut patch = MessageMetadataPatch::new();
    patch.set_preview(Some(replacement.clone()));

    metadata.apply_patch(patch);

    assert_eq!(metadata.preview(), Some(&replacement));
}

#[test]
fn complete_patch_without_a_preview_update_creates_a_message_without_one() {
    let message = complete_patch()
        .into_complete_message(folder_reference("account-1", "folder-1"), message_id())
        .expect("all selected fields are present");

    assert_eq!(message.metadata().preview(), None);
}

#[test]
fn complete_patch_with_a_preview_creates_a_message_with_it() {
    let preview = synthetic_preview("Synthetic opening line");
    let mut patch = complete_patch();
    patch.set_preview(Some(preview.clone()));

    let message = patch
        .into_complete_message(folder_reference("account-1", "folder-1"), message_id())
        .expect("all selected fields are present");

    assert_eq!(message.metadata().preview(), Some(&preview));
}

#[test]
fn complete_patch_with_a_cleared_preview_creates_a_message_without_one() {
    let mut patch = complete_patch();
    patch.set_preview(None);

    let message = patch
        .into_complete_message(folder_reference("account-1", "folder-1"), message_id())
        .expect("all selected fields are present");

    assert_eq!(message.metadata().preview(), None);
}

#[test]
fn preview_text_stays_out_of_debug_output() {
    let metadata = synthetic_metadata().with_preview(Some(synthetic_preview("preview-marker")));
    let mut patch = MessageMetadataPatch::new();
    patch.set_preview(Some(synthetic_preview("patch-preview-marker")));

    let debug = format!("{metadata:?}{patch:?}");

    assert!(!debug.contains("preview-marker"));
    assert!(debug.contains("MessagePreview(<redacted>)"));
}

fn message_id() -> crate::MessageId {
    crate::MessageId::try_from("message-1".to_owned()).expect("synthetic message ID is non-empty")
}

fn complete_patch() -> MessageMetadataPatch {
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(None);
    patch.set_from(None);
    patch.set_sender(None);
    patch.set_received_at(SystemTime::UNIX_EPOCH);
    patch.set_read_state(ReadState::Read);
    patch
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
