use crate::FolderId;

use super::{MailFolderKind, MailFolderKinds};

fn folder(id: &str) -> FolderId {
    FolderId::try_from(id.to_owned()).expect("synthetic folder ID is non-empty")
}

#[test]
fn an_empty_assignment_resolves_nothing() {
    let kinds = MailFolderKinds::new();

    assert_eq!(kinds.kind_of(&folder("folder-1")), None);
    assert_eq!(kinds.folder_of(MailFolderKind::Inbox), None);
}

#[test]
fn an_assigned_pair_resolves_in_both_directions() {
    let mut kinds = MailFolderKinds::new();

    assert!(kinds.assign(MailFolderKind::Archive, folder("folder-1")));

    assert_eq!(
        kinds.kind_of(&folder("folder-1")),
        Some(MailFolderKind::Archive)
    );
    assert_eq!(
        kinds.folder_of(MailFolderKind::Archive),
        Some(&folder("folder-1"))
    );
}

#[test]
fn a_kind_names_at_most_one_folder() {
    let mut kinds = MailFolderKinds::new();
    kinds.assign(MailFolderKind::Inbox, folder("folder-1"));

    assert!(!kinds.assign(MailFolderKind::Inbox, folder("folder-2")));

    assert_eq!(
        kinds.folder_of(MailFolderKind::Inbox),
        Some(&folder("folder-1"))
    );
    assert_eq!(kinds.kind_of(&folder("folder-2")), None);
}

#[test]
fn a_folder_has_at_most_one_kind() {
    let mut kinds = MailFolderKinds::new();
    kinds.assign(MailFolderKind::Inbox, folder("folder-1"));

    assert!(!kinds.assign(MailFolderKind::Drafts, folder("folder-1")));

    assert_eq!(
        kinds.kind_of(&folder("folder-1")),
        Some(MailFolderKind::Inbox)
    );
    assert_eq!(kinds.folder_of(MailFolderKind::Drafts), None);
}

#[test]
fn assigning_the_same_pair_twice_is_refused_without_a_change() {
    let mut kinds = MailFolderKinds::new();
    kinds.assign(MailFolderKind::SentItems, folder("folder-1"));
    let before = kinds.clone();

    assert!(!kinds.assign(MailFolderKind::SentItems, folder("folder-1")));

    assert_eq!(kinds, before);
}

#[test]
fn assignments_lists_each_accepted_pair_once() {
    let mut kinds = MailFolderKinds::new();
    kinds.assign(MailFolderKind::Inbox, folder("folder-1"));
    kinds.assign(MailFolderKind::Archive, folder("folder-2"));
    kinds.assign(MailFolderKind::Inbox, folder("folder-3"));

    let pairs: Vec<_> = kinds.assignments().collect();

    assert_eq!(pairs.len(), 2);
    assert!(pairs.contains(&(MailFolderKind::Inbox, &folder("folder-1"))));
    assert!(pairs.contains(&(MailFolderKind::Archive, &folder("folder-2"))));
}

#[test]
fn every_kind_can_name_its_own_folder() {
    let all = [
        MailFolderKind::Inbox,
        MailFolderKind::SentItems,
        MailFolderKind::Drafts,
        MailFolderKind::Archive,
        MailFolderKind::DeletedItems,
        MailFolderKind::JunkEmail,
    ];
    let mut kinds = MailFolderKinds::new();

    for (index, kind) in all.into_iter().enumerate() {
        assert!(kinds.assign(kind, folder(&format!("folder-{index}"))));
    }

    for (index, kind) in all.into_iter().enumerate() {
        let id = folder(&format!("folder-{index}"));
        assert_eq!(kinds.folder_of(kind), Some(&id));
        assert_eq!(kinds.kind_of(&id), Some(kind));
    }
}
