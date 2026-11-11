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
        "INSERT INTO mail_folders (account_id, folder_id, parent_folder_id, display_name)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (account_id, folder_id) DO UPDATE SET
            parent_folder_id = excluded.parent_folder_id,
            display_name = excluded.display_name",
        params![account_id, folder_id, parent_id, folder.display_name()],
    )?;
    transaction.execute(
        "INSERT INTO seen_mail_folders (folder_id) VALUES (?1)",
        [folder_id],
    )?;
    Ok(())
}

pub(super) fn read_folders(
    connection: &Connection,
    account_id: &str,
) -> CacheResult<Vec<MailFolder>> {
    let mut statement = connection.prepare(
        "SELECT folder_id, parent_folder_id, display_name
         FROM mail_folders WHERE account_id = ?1 ORDER BY display_name, folder_id",
    )?;
    let rows = statement.query_map([account_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let values = rows.collect::<Result<Vec<_>, _>>()?;
    values
        .into_iter()
        .map(|(folder_id, parent_id, display_name)| {
            let reference = FolderReference::new(
                AccountId::try_from(account_id.to_owned())
                    .map_err(|_| CacheFailure::InvalidData)?,
                FolderId::try_from(folder_id).map_err(|_| CacheFailure::InvalidData)?,
            );
            let parent = parent_id
                .map(|parent| {
                    FolderId::try_from(parent)
                        .map(|id| FolderReference::new(reference.account_id().clone(), id))
                        .map_err(|_| CacheFailure::InvalidData)
                })
                .transpose()?;
            MailFolder::new(reference, parent, display_name).map_err(|_| CacheFailure::InvalidData)
        })
        .collect()
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
