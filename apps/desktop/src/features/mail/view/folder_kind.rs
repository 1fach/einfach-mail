//! How the folder pane orders and illustrates folders by their kind.

use einfach_mail_domain::{MailFolder, MailFolderKind};

use crate::components::AppIcon;

/// The kinds the folder pane pins above every other folder, in the order it shows them.
pub(super) const PINNED_FOLDER_ORDER: [MailFolderKind; 6] = [
    MailFolderKind::Inbox,
    MailFolderKind::SentItems,
    MailFolderKind::Drafts,
    MailFolderKind::Archive,
    MailFolderKind::DeletedItems,
    MailFolderKind::JunkEmail,
];

/// The icon that marks a folder of `kind`. A folder with no kind gets the plain folder icon.
pub(super) fn folder_icon(kind: Option<MailFolderKind>) -> AppIcon {
    match kind {
        Some(MailFolderKind::Inbox) => AppIcon::Inbox,
        Some(MailFolderKind::SentItems) => AppIcon::SentItems,
        Some(MailFolderKind::Drafts) => AppIcon::Drafts,
        Some(MailFolderKind::Archive) => AppIcon::Archive,
        Some(MailFolderKind::DeletedItems) => AppIcon::DeletedItems,
        Some(MailFolderKind::JunkEmail) => AppIcon::JunkEmail,
        None => AppIcon::Folder,
    }
}

/// Puts the pinned folders first in [`PINNED_FOLDER_ORDER`] and keeps every other folder in the
/// order the cache returned it.
pub(super) fn navigation_order(mut folders: Vec<MailFolder>) -> Vec<MailFolder> {
    folders.sort_by_key(|folder| pinned_rank(folder.kind()));
    folders
}

fn pinned_rank(kind: Option<MailFolderKind>) -> usize {
    kind.and_then(|kind| {
        PINNED_FOLDER_ORDER
            .iter()
            .position(|pinned| *pinned == kind)
    })
    .unwrap_or(PINNED_FOLDER_ORDER.len())
}

#[cfg(test)]
mod tests;
