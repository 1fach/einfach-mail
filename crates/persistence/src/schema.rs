//! SQL schema and migrations for the encrypted mail cache.

use async_sqlite::Client;
use rusqlite_migration::{M, Migrations};

use crate::store::{CacheFailure, CacheResult};

pub(super) const MESSAGE_QUERY_VERSION: i64 = 1;

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(
        "CREATE TABLE mail_folders (
            account_id TEXT NOT NULL,
            folder_id TEXT NOT NULL,
            parent_folder_id TEXT,
            display_name TEXT NOT NULL,
            PRIMARY KEY (account_id, folder_id),
            FOREIGN KEY (account_id, parent_folder_id)
                REFERENCES mail_folders (account_id, folder_id)
                ON DELETE CASCADE
                DEFERRABLE INITIALLY DEFERRED
        );

        CREATE TABLE mail_messages (
            account_id TEXT NOT NULL,
            folder_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            subject TEXT,
            from_display_name TEXT,
            from_address TEXT,
            sender_display_name TEXT,
            sender_address TEXT,
            received_seconds INTEGER NOT NULL,
            received_nanoseconds INTEGER NOT NULL
                CHECK (received_nanoseconds BETWEEN 0 AND 999999999),
            is_read INTEGER NOT NULL CHECK (is_read IN (0, 1)),
            body_type TEXT CHECK (body_type IN ('html', 'text')),
            body_content TEXT,
            PRIMARY KEY (account_id, folder_id, message_id),
            FOREIGN KEY (account_id, folder_id)
                REFERENCES mail_folders (account_id, folder_id)
                ON DELETE CASCADE,
            CHECK (from_address IS NOT NULL OR from_display_name IS NULL),
            CHECK (sender_address IS NOT NULL OR sender_display_name IS NULL),
            CHECK ((body_type IS NULL) = (body_content IS NULL))
        );

        CREATE INDEX mail_messages_received_order
            ON mail_messages (
                account_id, folder_id, received_seconds DESC,
                received_nanoseconds DESC, message_id ASC
            );

        CREATE TABLE mail_account_sync_scope (
            account_id TEXT PRIMARY KEY,
            query_version INTEGER NOT NULL,
            cutoff_seconds INTEGER NOT NULL,
            cutoff_nanoseconds INTEGER NOT NULL
                CHECK (cutoff_nanoseconds BETWEEN 0 AND 999999999)
        );

        CREATE TABLE mail_sync_state (
            account_id TEXT NOT NULL,
            folder_id TEXT NOT NULL,
            delta_link TEXT,
            next_link TEXT,
            last_sync_seconds INTEGER,
            last_sync_nanoseconds INTEGER,
            PRIMARY KEY (account_id, folder_id),
            FOREIGN KEY (account_id, folder_id)
                REFERENCES mail_folders (account_id, folder_id)
                ON DELETE CASCADE,
            FOREIGN KEY (account_id)
                REFERENCES mail_account_sync_scope (account_id)
                ON DELETE CASCADE,
            CHECK ((last_sync_seconds IS NULL) = (last_sync_nanoseconds IS NULL)),
            CHECK (last_sync_nanoseconds IS NULL
                   OR last_sync_nanoseconds BETWEEN 0 AND 999999999),
            CHECK (last_sync_seconds IS NULL OR delta_link IS NOT NULL)
        );",
    )])
}

pub(super) async fn apply(client: &Client) -> CacheResult<()> {
    client
        .conn_mut_and_then(|connection| {
            migrations()
                .to_latest(connection)
                .map_err(|_| CacheFailure::Unavailable)
        })
        .await
        .map_err(|_| CacheFailure::Unavailable)?;
    Ok(())
}
