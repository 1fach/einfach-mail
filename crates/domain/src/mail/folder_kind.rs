use crate::FolderId;

/// The role a mailbox gives to one of its folders.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MailFolderKind {
    /// The folder that receives new mail.
    Inbox,
    /// The folder that holds sent messages.
    SentItems,
    /// The folder that holds unsent drafts.
    Drafts,
    /// The default folder used by Outlook's One_Click Archive action.
    Archive,
    /// The folder that holds messages deleted from ordinary folders.
    DeletedItems,
    /// The folder that holds messages classified as junk.
    JunkEmail,
}

/// A provider's assignment of folder kinds to folders.
///
/// Each kind names at most one folder, and each folder has at most one kind.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MailFolderKinds {
    assignments: Vec<(MailFolderKind, FolderId)>,
}

impl MailFolderKinds {
    /// Creates an assignment with no folders.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Assigns `kind` to `folder`.
    ///
    /// Returns `false` and leaves the assignment unchanged when `kind` already names a folder
    /// or `folder` already has a kind.
    pub fn assign(&mut self, kind: MailFolderKind, folder: FolderId) -> bool {
        let taken = self
            .assignments
            .iter()
            .any(|(assigned_kind, assigned_folder)| {
                *assigned_kind == kind || *assigned_folder == folder
            });
        if !taken {
            self.assignments.push((kind, folder));
        }
        !taken
    }

    /// Returns the kind assigned to `folder`, when it has one.
    #[must_use]
    pub fn kind_of(&self, folder: &FolderId) -> Option<MailFolderKind> {
        self.assignments
            .iter()
            .find_map(|(kind, assigned)| (assigned == folder).then_some(*kind))
    }

    /// Returns the folder assigned to `kind`, when there is one.
    #[must_use]
    pub fn folder_of(&self, kind: MailFolderKind) -> Option<&FolderId> {
        self.assignments
            .iter()
            .find_map(|(assigned_kind, folder)| (*assigned_kind == kind).then_some(folder))
    }
}

#[cfg(test)]
mod tests;
