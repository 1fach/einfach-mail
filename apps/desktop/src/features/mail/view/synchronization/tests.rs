use einfach_mail_domain::{AccountId, FolderId, FolderReference, MailFolder, MailFolderKind};

use super::preferred_folder;

fn folder(id: &str, name: &str, kind: Option<MailFolderKind>) -> MailFolder {
    let account = AccountId::try_from("account-1".to_owned()).expect("synthetic account ID");
    let reference = FolderReference::new(
        account,
        FolderId::try_from(id.to_owned()).expect("synthetic folder ID is valid"),
    );
    MailFolder::new(reference, None, name.to_owned())
        .expect("synthetic folder is valid")
        .with_kind(kind)
}

fn preferred_id(folders: &[MailFolder]) -> String {
    preferred_folder(folders)
        .expect("a folder is preferred")
        .folder_id()
        .as_str()
        .to_owned()
}

#[test]
fn localized_inbox_is_selected_by_kind() {
    let folders = [
        folder("archive", "Archiv", Some(MailFolderKind::Archive)),
        folder("inbox", "Posteingang", Some(MailFolderKind::Inbox)),
    ];

    assert_eq!(preferred_id(&folders), "inbox");
}

#[test]
fn inbox_kind_wins_over_a_folder_named_inbox() {
    let folders = [
        folder("named", "Inbox", None),
        folder("typed", "Posteingang", Some(MailFolderKind::Inbox)),
    ];

    assert_eq!(preferred_id(&folders), "typed");
}

#[test]
fn name_match_is_used_when_no_kind_is_resolved() {
    let folders = [
        folder("archive", "Archive", None),
        folder("inbox", "INBOX", None),
    ];

    assert_eq!(preferred_id(&folders), "inbox");
}

#[test]
fn first_folder_is_used_without_kind_or_name_match() {
    let folders = [folder("a", "Projects", None), folder("b", "Receipts", None)];

    assert_eq!(preferred_id(&folders), "a");
    assert!(preferred_folder(&[]).is_none());
}
