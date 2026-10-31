//! Provider-independent mail concepts used by cached browsing.

use std::{fmt, time::SystemTime};

use crate::{AccountId, FolderId, MessageId};

/// An account-scoped reference to a provider folder.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FolderReference {
    account_id: AccountId,
    folder_id: FolderId,
}

impl FolderReference {
    /// Creates a folder reference from its account and provider-issued ID.
    #[must_use]
    pub fn new(account_id: AccountId, folder_id: FolderId) -> Self {
        Self {
            account_id,
            folder_id,
        }
    }

    /// Returns the account that scopes this folder reference.
    #[must_use]
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the provider-issued folder ID.
    #[must_use]
    pub fn folder_id(&self) -> &FolderId {
        &self.folder_id
    }
}

/// The parent reference is invalid for this folder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum FolderParentError {
    /// The parent folder belongs to another account.
    AccountMismatch,
    /// A folder cannot be its own parent.
    SelfParent,
}

impl fmt::Display for FolderParentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AccountMismatch => "parent folder must belong to the same account",
            Self::SelfParent => "a folder cannot be its own parent",
        })
    }
}

impl std::error::Error for FolderParentError {}

/// The mailbox address contains no non-whitespace characters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct EmptyMailboxAddress;

impl fmt::Display for EmptyMailboxAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mailbox address must not be empty")
    }
}

impl std::error::Error for EmptyMailboxAddress {}

/// A mail folder and its account-scoped location in the folder tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailFolder {
    reference: FolderReference,
    parent: Option<FolderReference>,
    display_name: String,
}

impl MailFolder {
    /// Creates a folder with its account-scoped identity and display metadata.
    ///
    /// # Errors
    ///
    /// Returns [`FolderParentError`] if the parent belongs to another account or is this folder.
    pub fn new(
        reference: FolderReference,
        parent: Option<FolderReference>,
        display_name: String,
    ) -> Result<Self, FolderParentError> {
        if let Some(parent) = parent.as_ref() {
            if parent.account_id() != reference.account_id() {
                return Err(FolderParentError::AccountMismatch);
            }
            if parent.folder_id() == reference.folder_id() {
                return Err(FolderParentError::SelfParent);
            }
        }

        Ok(Self {
            reference,
            parent,
            display_name,
        })
    }

    /// Returns this folder's account-scoped reference.
    #[must_use]
    pub fn reference(&self) -> &FolderReference {
        &self.reference
    }

    /// Returns the account-scoped parent reference, if this folder has one.
    #[must_use]
    pub fn parent(&self) -> Option<&FolderReference> {
        self.parent.as_ref()
    }

    /// Returns the folder's display name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}

/// A mailbox identity associated with a message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mailbox {
    display_name: Option<String>,
    address: String,
}

impl Mailbox {
    /// Creates a mailbox identity from a non-empty address and optional display name.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyMailboxAddress`] if the address is empty or contains only whitespace.
    pub fn try_new(
        address: String,
        display_name: Option<String>,
    ) -> Result<Self, EmptyMailboxAddress> {
        if address.trim().is_empty() {
            return Err(EmptyMailboxAddress);
        }

        Ok(Self {
            display_name,
            address,
        })
    }

    /// Returns the display name, when one is available.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Returns the mailbox address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }
}

/// The distinct `from` and `sender` mailbox roles attached to a message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageOrigin {
    from: Option<Mailbox>,
    sender: Option<Mailbox>,
}

impl MessageOrigin {
    /// Creates message origin metadata while preserving the two mailbox roles.
    #[must_use]
    pub fn new(from: Option<Mailbox>, sender: Option<Mailbox>) -> Self {
        Self { from, sender }
    }

    /// Returns the mailbox the message is from, when available.
    #[must_use]
    pub fn from_mailbox(&self) -> Option<&Mailbox> {
        self.from.as_ref()
    }

    /// Returns the actual sender, when available.
    #[must_use]
    pub fn sender(&self) -> Option<&Mailbox> {
        self.sender.as_ref()
    }
}

/// Whether a message is read or unread in the synchronized mailbox state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReadState {
    /// The message is read.
    Read,
    /// The message is unread.
    Unread,
}

/// Metadata shown when browsing a message list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageMetadata {
    subject: Option<String>,
    origin: MessageOrigin,
    received_at: SystemTime,
    read_state: ReadState,
}

impl MessageMetadata {
    /// Creates message-list metadata without including message body content.
    #[must_use]
    pub fn new(
        subject: Option<String>,
        origin: MessageOrigin,
        received_at: SystemTime,
        read_state: ReadState,
    ) -> Self {
        Self {
            subject,
            origin,
            received_at,
            read_state,
        }
    }

    /// Returns the subject, when one is available.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    /// Returns the message's `from` and `sender` mailboxes.
    #[must_use]
    pub fn origin(&self) -> &MessageOrigin {
        &self.origin
    }

    /// Returns the message's received instant.
    #[must_use]
    pub fn received_at(&self) -> &SystemTime {
        &self.received_at
    }

    /// Returns the synchronized read state.
    #[must_use]
    pub fn read_state(&self) -> ReadState {
        self.read_state
    }

    /// Applies the changed fields in a message metadata patch.
    pub fn apply_patch(&mut self, patch: MessageMetadataPatch) {
        patch.subject.apply_to(&mut self.subject);
        patch.from.apply_to(&mut self.origin.from);
        patch.sender.apply_to(&mut self.origin.sender);
        patch.received_at.apply_to(&mut self.received_at);
        patch.read_state.apply_to(&mut self.read_state);
    }
}

/// A provider-independent set of message metadata fields to update.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MessageMetadataPatch {
    subject: FieldUpdate<Option<String>>,
    from: FieldUpdate<Option<Mailbox>>,
    sender: FieldUpdate<Option<Mailbox>>,
    received_at: FieldUpdate<SystemTime>,
    read_state: FieldUpdate<ReadState>,
}

impl MessageMetadataPatch {
    /// Creates a patch that leaves every metadata field unchanged.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces or clears the subject when the patch is applied.
    pub fn set_subject(&mut self, subject: Option<String>) {
        self.subject = FieldUpdate::Set(subject);
    }

    /// Replaces or clears the `from` mailbox when the patch is applied.
    pub fn set_from(&mut self, from: Option<Mailbox>) {
        self.from = FieldUpdate::Set(from);
    }

    /// Replaces or clears the sender mailbox when the patch is applied.
    pub fn set_sender(&mut self, sender: Option<Mailbox>) {
        self.sender = FieldUpdate::Set(sender);
    }

    /// Replaces the received time when the patch is applied.
    pub fn set_received_at(&mut self, received_at: SystemTime) {
        self.received_at = FieldUpdate::Set(received_at);
    }

    /// Replaces the read state when the patch is applied.
    pub fn set_read_state(&mut self, read_state: ReadState) {
        self.read_state = FieldUpdate::Set(read_state);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum FieldUpdate<T> {
    #[default]
    Unchanged,
    Set(T),
}

impl<T> FieldUpdate<T> {
    fn apply_to(self, field: &mut T) {
        if let Self::Set(value) = self {
            *field = value;
        }
    }
}

/// A message's account- and folder-scoped identity and browsing metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailMessage {
    folder: FolderReference,
    id: MessageId,
    metadata: MessageMetadata,
}

impl MailMessage {
    /// Creates a message record within an account-scoped folder.
    #[must_use]
    pub fn new(folder: FolderReference, id: MessageId, metadata: MessageMetadata) -> Self {
        Self {
            folder,
            id,
            metadata,
        }
    }

    /// Returns the account-scoped folder containing this message.
    #[must_use]
    pub fn folder(&self) -> &FolderReference {
        &self.folder
    }

    /// Returns the provider-issued message identifier.
    #[must_use]
    pub fn id(&self) -> &MessageId {
        &self.id
    }

    /// Returns the metadata used to browse this message.
    #[must_use]
    pub fn metadata(&self) -> &MessageMetadata {
        &self.metadata
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::{AccountId, FolderId, FolderReference};

    use super::{
        EmptyMailboxAddress, FolderParentError, MailFolder, Mailbox, MessageMetadata,
        MessageMetadataPatch, MessageOrigin, ReadState,
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
    fn mailbox_requires_a_non_empty_address() {
        for address in [String::new(), " \t\n".to_owned()] {
            assert_eq!(Mailbox::try_new(address, None), Err(EmptyMailboxAddress));
        }
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

        metadata.apply_patch(patch);

        assert_eq!(metadata.subject(), None);
        assert_eq!(metadata.origin().from_mailbox(), None);
        assert_eq!(metadata.origin().sender(), Some(&replacement_sender));
        assert_eq!(metadata.received_at(), &replacement_time);
        assert_eq!(metadata.read_state(), ReadState::Unread);
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
        )
    }

    fn synthetic_mailbox(address: &str) -> Mailbox {
        Mailbox::try_new(address.to_owned(), None).expect("synthetic mailbox address is valid")
    }
}
