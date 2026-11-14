use einfach_mail_domain::{AccountId, FolderId, FolderReference, MailFolder, MailFolderKind};

use super::{PINNED_FOLDER_ORDER, folder_icon, navigation_order};
use crate::components::AppIcon;

const KIND_COUNT: usize = 6;

/// The only exhaustive list of kinds in these tests. Adding a kind stops this from compiling until
/// the pinned order and the icon table name it.
fn position_in_declaration(kind: MailFolderKind) -> usize {
    match kind {
        MailFolderKind::Inbox => 0,
        MailFolderKind::SentItems => 1,
        MailFolderKind::Drafts => 2,
        MailFolderKind::Archive => 3,
        MailFolderKind::DeletedItems => 4,
        MailFolderKind::JunkEmail => 5,
    }
}

fn folder(id: &str, kind: Option<MailFolderKind>) -> MailFolder {
    let account = AccountId::try_from("account-1".to_owned()).expect("synthetic account ID");
    let reference = FolderReference::new(
        account,
        FolderId::try_from(id.to_owned()).expect("synthetic folder ID is valid"),
    );
    MailFolder::new(reference, None, id.to_owned())
        .expect("synthetic folder is valid")
        .with_kind(kind)
}

fn ids(folders: &[MailFolder]) -> Vec<&str> {
    folders
        .iter()
        .map(|folder| folder.reference().folder_id().as_str())
        .collect()
}

#[test]
fn the_pinned_order_names_every_kind_exactly_once() {
    let mut seen = [0_usize; KIND_COUNT];
    for kind in PINNED_FOLDER_ORDER {
        seen[position_in_declaration(kind)] += 1;
    }

    assert_eq!(seen, [1; KIND_COUNT]);
    assert_eq!(PINNED_FOLDER_ORDER.len(), KIND_COUNT);
}

#[test]
fn each_kind_maps_to_its_own_icon_and_an_unkinded_folder_gets_the_plain_folder() {
    let expected = [
        (MailFolderKind::Inbox, AppIcon::Inbox),
        (MailFolderKind::SentItems, AppIcon::SentItems),
        (MailFolderKind::Drafts, AppIcon::Drafts),
        (MailFolderKind::Archive, AppIcon::Archive),
        (MailFolderKind::DeletedItems, AppIcon::DeletedItems),
        (MailFolderKind::JunkEmail, AppIcon::JunkEmail),
    ];

    assert_eq!(expected.len(), KIND_COUNT);
    for (kind, icon) in expected {
        assert_eq!(folder_icon(Some(kind)), icon, "{kind:?}");
    }
    assert_eq!(folder_icon(None), AppIcon::Folder);
}

#[test]
fn pinned_folders_come_first_in_wino_order_and_the_rest_keep_cache_order() {
    let cache_order = vec![
        folder("projects", None),
        folder("junk", Some(MailFolderKind::JunkEmail)),
        folder("inbox", Some(MailFolderKind::Inbox)),
        folder("receipts", None),
        folder("archive", Some(MailFolderKind::Archive)),
        folder("deleted", Some(MailFolderKind::DeletedItems)),
        folder("sent", Some(MailFolderKind::SentItems)),
        folder("drafts", Some(MailFolderKind::Drafts)),
        folder("clients", None),
    ];

    let ordered = navigation_order(cache_order);

    assert_eq!(
        ids(&ordered),
        [
            "inbox", "sent", "drafts", "archive", "deleted", "junk", "projects", "receipts",
            "clients"
        ]
    );
}

#[test]
fn folders_without_kinds_keep_the_order_the_cache_returned() {
    let cache_order = vec![folder("b", None), folder("a", None), folder("c", None)];

    assert_eq!(ids(&navigation_order(cache_order)), ["b", "a", "c"]);
}
