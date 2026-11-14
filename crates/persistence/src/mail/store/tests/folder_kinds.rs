use super::*;

const ALL_KINDS: [MailFolderKind; 6] = [
    MailFolderKind::Inbox,
    MailFolderKind::SentItems,
    MailFolderKind::Drafts,
    MailFolderKind::Archive,
    MailFolderKind::DeletedItems,
    MailFolderKind::JunkEmail,
];

fn folder_id(id: &str) -> FolderId {
    FolderId::try_from(id.to_owned()).expect("synthetic folder ID is valid")
}

fn plain_folder(id: &str) -> MailFolder {
    MailFolder::new(
        FolderReference::new(account_id(), folder_id(id)),
        None,
        "Synthetic folder".to_owned(),
    )
    .expect("synthetic folder is valid")
}

fn kinds(assignments: &[(MailFolderKind, &str)]) -> MailFolderKinds {
    let mut kinds = MailFolderKinds::new();
    for (kind, id) in assignments {
        assert!(
            kinds.assign(*kind, folder_id(id)),
            "synthetic assignments are one-to-one"
        );
    }
    kinds
}

/// Returns the kind of each cached folder, ordered by folder ID.
async fn cached_kinds(cache: &SqliteAccountCache) -> Vec<(String, Option<MailFolderKind>)> {
    let mut cached: Vec<_> = cache
        .list_folders(&account_id())
        .await
        .expect("cached folders are readable")
        .into_iter()
        .map(|folder| {
            (
                folder.reference().folder_id().as_str().to_owned(),
                folder.kind(),
            )
        })
        .collect();
    cached.sort_by(|left, right| left.0.cmp(&right.0));
    cached
}

async fn has_unresolved_kinds(cache: &SqliteAccountCache) -> bool {
    cache
        .has_unresolved_folder_kinds(&account_id())
        .await
        .expect("cached folder kinds are readable")
}

#[test]
fn a_resolution_names_its_folders_and_resolves_every_other_folder_as_ordinary() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache = cache_with_folders(
            &directory,
            vec![plain_folder("a"), plain_folder("b"), plain_folder("c")],
        )
        .await;
        assert!(has_unresolved_kinds(&cache).await);

        cache
            .store_folder_kinds(
                &account_id(),
                kinds(&[
                    (MailFolderKind::Inbox, "a"),
                    (MailFolderKind::Archive, "b"),
                    (MailFolderKind::Drafts, "not-cached"),
                ]),
            )
            .await
            .expect("synthetic resolution is stored");

        assert_eq!(
            cached_kinds(&cache).await,
            [
                ("a".to_owned(), Some(MailFolderKind::Inbox)),
                ("b".to_owned(), Some(MailFolderKind::Archive)),
                ("c".to_owned(), None),
            ]
        );
        assert!(!has_unresolved_kinds(&cache).await);
        cache.close_for_tests().await;
    });
}

#[test]
fn every_kind_survives_storage() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let ids = ["a", "b", "c", "d", "e", "f"];
        let cache =
            cache_with_folders(&directory, ids.into_iter().map(plain_folder).collect()).await;
        let assignments: Vec<_> = ALL_KINDS.into_iter().zip(ids).collect();

        cache
            .store_folder_kinds(&account_id(), kinds(&assignments))
            .await
            .expect("synthetic resolution is stored");

        let expected: Vec<_> = assignments
            .into_iter()
            .map(|(kind, id)| (id.to_owned(), Some(kind)))
            .collect();
        assert_eq!(cached_kinds(&cache).await, expected);
        cache.close_for_tests().await;
    });
}

#[test]
fn replacing_the_folders_keeps_stored_kinds_and_leaves_a_new_folder_unresolved() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache =
            cache_with_folders(&directory, vec![plain_folder("a"), plain_folder("b")]).await;
        cache
            .store_folder_kinds(&account_id(), kinds(&[(MailFolderKind::Inbox, "a")]))
            .await
            .expect("synthetic resolution is stored");

        cache
            .replace_folders(
                &account_id(),
                vec![
                    plain_folder("a"),
                    plain_folder("b").with_kind(Some(MailFolderKind::Drafts)),
                    plain_folder("c").with_kind(Some(MailFolderKind::Archive)),
                ],
            )
            .await
            .expect("a later folder listing is cached");

        assert_eq!(
            cached_kinds(&cache).await,
            [
                ("a".to_owned(), Some(MailFolderKind::Inbox)),
                ("b".to_owned(), None),
                ("c".to_owned(), None),
            ]
        );
        assert!(has_unresolved_kinds(&cache).await);
        cache.close_for_tests().await;
    });
}

#[test]
fn a_later_resolution_moves_a_kind_to_another_folder() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache =
            cache_with_folders(&directory, vec![plain_folder("a"), plain_folder("b")]).await;
        cache
            .store_folder_kinds(&account_id(), kinds(&[(MailFolderKind::Archive, "a")]))
            .await
            .expect("first synthetic resolution is stored");

        cache
            .store_folder_kinds(&account_id(), kinds(&[(MailFolderKind::Archive, "b")]))
            .await
            .expect("second synthetic resolution is stored");

        assert_eq!(
            cached_kinds(&cache).await,
            [
                ("a".to_owned(), None),
                ("b".to_owned(), Some(MailFolderKind::Archive)),
            ]
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn an_account_without_folders_has_no_unresolved_kind() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache = cache_with_folders(&directory, Vec::new()).await;

        assert!(!has_unresolved_kinds(&cache).await);
        cache.close_for_tests().await;
    });
}

#[test]
fn folder_kind_operations_reject_another_account() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache = cache_with_folders(&directory, vec![plain_folder("a")]).await;
        let other_account =
            AccountId::try_from("other-account".to_owned()).expect("synthetic account ID is valid");

        assert_eq!(
            cache.has_unresolved_folder_kinds(&other_account).await,
            Err(MailCacheError::AccountMismatch)
        );
        assert_eq!(
            cache
                .store_folder_kinds(&other_account, kinds(&[(MailFolderKind::Inbox, "a")]))
                .await,
            Err(MailCacheError::AccountMismatch)
        );
        assert!(has_unresolved_kinds(&cache).await);
        cache.close_for_tests().await;
    });
}
