use super::*;

pub(super) fn validate_folder_snapshot(
    account_id: &AccountId,
    folders: &[MailFolder],
) -> Result<(), MailCacheError> {
    let mut folder_ids = HashSet::with_capacity(folders.len());
    for folder in folders {
        if folder.reference().account_id() != account_id
            || !folder_ids.insert(folder.reference().folder_id().as_str())
        {
            return Err(MailCacheError::InvalidData);
        }
    }
    for folder in folders {
        if folder
            .parent()
            .is_some_and(|parent| !folder_ids.contains(parent.folder_id().as_str()))
        {
            return Err(MailCacheError::InvalidData);
        }
    }
    Ok(())
}

pub(super) fn replace_folder_rows(
    transaction: &Transaction<'_>,
    account_id: &str,
    folders: Vec<MailFolder>,
) -> CacheResult<()> {
    transaction.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS seen_mail_folders (
            folder_id TEXT PRIMARY KEY
        );
        DELETE FROM seen_mail_folders;",
    )?;
    for folder in folders {
        insert_folder(transaction, account_id, folder)?;
    }
    transaction.execute(
        "DELETE FROM mail_folders
         WHERE account_id = ?1
           AND folder_id NOT IN (SELECT folder_id FROM seen_mail_folders)",
        [account_id],
    )?;
    Ok(())
}

fn insert_folder(
    transaction: &Transaction<'_>,
    account_id: &str,
    folder: MailFolder,
) -> CacheResult<()> {
    let folder_id = folder.reference().folder_id().as_str();
    let parent_id = folder.parent().map(|parent| parent.folder_id().as_str());
    transaction.execute(
        "INSERT INTO mail_folders (
            account_id, folder_id, parent_folder_id, display_name, unread_reported
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (account_id, folder_id) DO UPDATE SET
            parent_folder_id = excluded.parent_folder_id,
            display_name = excluded.display_name,
            unread_reported = excluded.unread_reported",
        params![
            account_id,
            folder_id,
            parent_id,
            folder.display_name(),
            folder.unread_count().map(UnreadCount::get)
        ],
    )?;
    transaction.execute(
        "INSERT INTO seen_mail_folders (folder_id) VALUES (?1)",
        [folder_id],
    )?;
    Ok(())
}

/// Counts the cached unread messages of the `mail_folders` row that a statement names `f`.
///
/// [`READ_FOLDERS_SQL`] adds this count and [`REBASELINE_UNREAD_SQL`] subtracts it. The two
/// statements must count the same rows, or the unread count that a folder shows drifts from
/// the provider's count.
const CACHED_UNREAD_SQL: &str = "(
    SELECT COUNT(*) FROM mail_messages m
    WHERE m.account_id = f.account_id AND m.folder_id = f.folder_id AND m.is_read = 0
)";

/// Selects an account's folders and the unread count to show for each.
///
/// The count is `unread_uncached` plus the cached unread messages. It is NULL while
/// `unread_uncached` is NULL.
static READ_FOLDERS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT f.folder_id, f.parent_folder_id, f.display_name, f.kind,
                f.unread_uncached + {CACHED_UNREAD_SQL} AS unread_count
         FROM mail_folders f
         WHERE f.account_id = ?1
         ORDER BY f.display_name, f.folder_id"
    )
});

/// Sets `unread_uncached` to the provider's count minus the cached unread messages.
///
/// The result is NULL when the provider reported no count, and 0 when the cache holds more
/// unread messages than the provider reported.
static REBASELINE_UNREAD_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "UPDATE mail_folders AS f
         SET unread_uncached = MAX(0, f.unread_reported - {CACHED_UNREAD_SQL})
         WHERE f.account_id = ?1 AND f.folder_id = ?2"
    )
});

pub(super) fn rebaseline_uncached_unread(
    transaction: &Transaction<'_>,
    folder: &FolderReference,
) -> CacheResult<()> {
    transaction.execute(
        &REBASELINE_UNREAD_SQL,
        params![folder.account_id().as_str(), folder.folder_id().as_str()],
    )?;
    Ok(())
}

/// The stored kind of a folder that a resolution found to be an ordinary folder.
///
/// A NULL kind means that no resolution has covered the folder yet.
const ORDINARY_FOLDER_KIND: &str = "other";

fn folder_kind_name(kind: MailFolderKind) -> &'static str {
    match kind {
        MailFolderKind::Inbox => "inbox",
        MailFolderKind::SentItems => "sent_items",
        MailFolderKind::Drafts => "drafts",
        MailFolderKind::Archive => "archive",
        MailFolderKind::DeletedItems => "deleted_items",
        MailFolderKind::JunkEmail => "junk_email",
    }
}

fn folder_kind_from_name(name: &str) -> CacheResult<Option<MailFolderKind>> {
    match name {
        ORDINARY_FOLDER_KIND => Ok(None),
        "inbox" => Ok(Some(MailFolderKind::Inbox)),
        "sent_items" => Ok(Some(MailFolderKind::SentItems)),
        "drafts" => Ok(Some(MailFolderKind::Drafts)),
        "archive" => Ok(Some(MailFolderKind::Archive)),
        "deleted_items" => Ok(Some(MailFolderKind::DeletedItems)),
        "junk_email" => Ok(Some(MailFolderKind::JunkEmail)),
        _ => Err(CacheFailure::InvalidData),
    }
}

pub(super) fn has_unresolved_folder_kinds(
    connection: &Connection,
    account_id: &str,
) -> CacheResult<bool> {
    let unresolved = connection.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM mail_folders WHERE account_id = ?1 AND kind IS NULL
        )",
        [account_id],
        |row| row.get(0),
    )?;
    Ok(unresolved)
}

pub(super) fn store_folder_kind_rows(
    transaction: &Transaction<'_>,
    account_id: &str,
    kinds: &MailFolderKinds,
) -> CacheResult<()> {
    // Every folder becomes ordinary first. A kind that moves to another folder would
    // otherwise break the one-folder-per-kind index while its earlier folder still has it.
    transaction.execute(
        "UPDATE mail_folders SET kind = ?2 WHERE account_id = ?1",
        params![account_id, ORDINARY_FOLDER_KIND],
    )?;
    for (kind, folder_id) in kinds.assignments() {
        transaction.execute(
            "UPDATE mail_folders SET kind = ?3 WHERE account_id = ?1 AND folder_id = ?2",
            params![account_id, folder_id.as_str(), folder_kind_name(kind)],
        )?;
    }
    Ok(())
}

struct RawFolder {
    folder_id: String,
    parent_folder_id: Option<String>,
    display_name: String,
    kind: Option<String>,
    unread_count: Option<i64>,
}

pub(super) fn read_folders(
    connection: &Connection,
    account_id: &str,
) -> CacheResult<Vec<MailFolder>> {
    let account_id =
        AccountId::try_from(account_id.to_owned()).map_err(|_| CacheFailure::InvalidData)?;
    let mut statement = connection.prepare(&READ_FOLDERS_SQL)?;
    let rows = statement.query_map([account_id.as_str()], |row| {
        Ok(RawFolder {
            folder_id: row.get("folder_id")?,
            parent_folder_id: row.get("parent_folder_id")?,
            display_name: row.get("display_name")?,
            kind: row.get("kind")?,
            unread_count: row.get("unread_count")?,
        })
    })?;
    rows.map(|row| decode_folder(&account_id, row?)).collect()
}

fn decode_folder(account_id: &AccountId, raw: RawFolder) -> CacheResult<MailFolder> {
    let reference = folder_reference(account_id, raw.folder_id)?;
    let parent = raw
        .parent_folder_id
        .map(|parent_id| folder_reference(account_id, parent_id))
        .transpose()?;
    let kind = raw
        .kind
        .as_deref()
        .map(folder_kind_from_name)
        .transpose()?
        .flatten();
    let unread_count = raw
        .unread_count
        .map(|count| u32::try_from(count).map(UnreadCount::new))
        .transpose()
        .map_err(|_| CacheFailure::InvalidData)?;
    let folder = MailFolder::new(reference, parent, raw.display_name)
        .map_err(|_| CacheFailure::InvalidData)?;
    Ok(folder.with_kind(kind).with_unread_count(unread_count))
}

fn folder_reference(account_id: &AccountId, folder_id: String) -> CacheResult<FolderReference> {
    let folder_id = FolderId::try_from(folder_id).map_err(|_| CacheFailure::InvalidData)?;
    Ok(FolderReference::new(account_id.clone(), folder_id))
}

pub(super) fn folder_exists(
    connection: &Connection,
    folder: &FolderReference,
) -> CacheResult<bool> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM mail_folders WHERE account_id = ?1 AND folder_id = ?2",
            params![folder.account_id().as_str(), folder.folder_id().as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}
