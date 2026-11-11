use async_sqlite::rusqlite::Connection;

use super::migrations;
use crate::shared::store::CacheFailure;

const FLAG_SCHEMA_VERSION: usize = 5;
const REPLY_SCHEMA_BASE_VERSION: usize = 7;
const REPLY_SCHEMA_REPAIRED_VERSION: i64 = 8;

pub(super) const FLAG_SCHEMA_TRANSITION_SQL: &str =
    "ALTER TABLE mail_messages ADD COLUMN flag_status TEXT
    CHECK (flag_status IS NULL OR flag_status IN (
        'not_flagged', 'flagged', 'complete'
    ));

    UPDATE mail_account_sync_scope SET query_version = 2;

    UPDATE mail_sync_state
    SET delta_link = NULL,
        next_link = NULL,
        last_sync_seconds = NULL,
        last_sync_nanoseconds = NULL,
        full_sync_generation = 0,
        full_sync_active = 0;

    UPDATE mail_messages SET full_sync_generation = NULL;";

pub(super) const FOLDER_WRITE_SYNC_REQUIREMENT_TABLE_SQL: &str =
    "CREATE TABLE mail_folder_write_sync_requirement (
    account_id TEXT NOT NULL,
    folder_id TEXT NOT NULL,
    PRIMARY KEY (account_id, folder_id),
    FOREIGN KEY (account_id, folder_id)
        REFERENCES mail_folders (account_id, folder_id)
        ON DELETE CASCADE
);";

pub(super) fn apply_parallel_schema_compatibility(
    connection: &mut Connection,
) -> Result<(), CacheFailure> {
    let version = migrations()
        .current_version(connection)
        .map_err(|_| CacheFailure::Unavailable)?;

    if usize::from(version) == REPLY_SCHEMA_BASE_VERSION
        && has_column(connection, "mail_pending_send", "target_kind")?
    {
        return repair_reply_schema(connection);
    }

    if usize::from(version) == FLAG_SCHEMA_VERSION {
        repair_flag_schema(connection)?;
    }
    Ok(())
}

fn repair_reply_schema(connection: &mut Connection) -> Result<(), CacheFailure> {
    let transaction = connection
        .transaction()
        .map_err(|_| CacheFailure::Unavailable)?;
    if !has_table(&transaction, "mail_folder_write_sync_requirement")? {
        transaction
            .execute_batch(FOLDER_WRITE_SYNC_REQUIREMENT_TABLE_SQL)
            .map_err(|_| CacheFailure::Unavailable)?;
    }
    transaction
        .pragma_update(None, "user_version", REPLY_SCHEMA_REPAIRED_VERSION)
        .map_err(|_| CacheFailure::Unavailable)?;
    transaction.commit().map_err(|_| CacheFailure::Unavailable)
}

fn repair_flag_schema(connection: &mut Connection) -> Result<(), CacheFailure> {
    if has_column(connection, "mail_messages", "flag_status")? {
        return Ok(());
    }

    let transaction = connection
        .transaction()
        .map_err(|_| CacheFailure::Unavailable)?;
    transaction
        .execute_batch(FLAG_SCHEMA_TRANSITION_SQL)
        .map_err(|_| CacheFailure::Unavailable)?;
    transaction.commit().map_err(|_| CacheFailure::Unavailable)
}

fn has_table(connection: &Connection, table: &str) -> Result<bool, CacheFailure> {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
            )",
            [table],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| CacheFailure::Unavailable)
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, CacheFailure> {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2
            )",
            async_sqlite::rusqlite::params![table, column],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| CacheFailure::Unavailable)
}
