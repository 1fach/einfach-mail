//! SQL schema and migrations for the encrypted mail cache.

use async_sqlite::{Client, rusqlite::Connection};
use rusqlite_migration::{M, Migrations};

use crate::store::{CacheFailure, CacheResult};

pub(super) const MESSAGE_QUERY_VERSION: i64 = 2;

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
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
        ),
        M::up(
            "ALTER TABLE mail_sync_state
            ADD COLUMN full_sync_generation INTEGER NOT NULL DEFAULT 0
            CHECK (full_sync_generation >= 0);

        ALTER TABLE mail_sync_state
            ADD COLUMN full_sync_active INTEGER NOT NULL DEFAULT 0
            CHECK (full_sync_active IN (0, 1));

        ALTER TABLE mail_messages
            ADD COLUMN full_sync_generation INTEGER
            CHECK (full_sync_generation IS NULL OR full_sync_generation >= 0);",
        ),
        M::up(
            "ALTER TABLE mail_sync_state
            ADD COLUMN checkpoint_revision INTEGER NOT NULL DEFAULT 0
            CHECK (checkpoint_revision >= 0);",
        ),
        M::up(
            "CREATE TABLE calendar_view_sync_state (
                account_id TEXT NOT NULL,
                range_start_seconds INTEGER NOT NULL,
                range_start_nanoseconds INTEGER NOT NULL
                    CHECK (range_start_nanoseconds BETWEEN 0 AND 999999999),
                range_end_seconds INTEGER NOT NULL,
                range_end_nanoseconds INTEGER NOT NULL
                    CHECK (range_end_nanoseconds BETWEEN 0 AND 999999999),
                local_start_julian_day INTEGER NOT NULL,
                local_end_julian_day INTEGER NOT NULL,
                delta_link TEXT,
                next_link TEXT,
                last_sync_seconds INTEGER,
                last_sync_nanoseconds INTEGER,
                PRIMARY KEY (
                    account_id,
                    range_start_seconds,
                    range_start_nanoseconds,
                    range_end_seconds,
                    range_end_nanoseconds,
                    local_start_julian_day,
                    local_end_julian_day
                ),
                CHECK (
                    range_start_seconds < range_end_seconds
                    OR (range_start_seconds = range_end_seconds
                        AND range_start_nanoseconds < range_end_nanoseconds)
                ),
                CHECK (local_start_julian_day < local_end_julian_day),
                CHECK ((last_sync_seconds IS NULL) = (last_sync_nanoseconds IS NULL)),
                CHECK (last_sync_nanoseconds IS NULL
                       OR last_sync_nanoseconds BETWEEN 0 AND 999999999),
                CHECK (last_sync_seconds IS NULL OR delta_link IS NOT NULL)
            );

            CREATE TABLE calendar_events (
                account_id TEXT NOT NULL,
                range_start_seconds INTEGER NOT NULL,
                range_start_nanoseconds INTEGER NOT NULL,
                range_end_seconds INTEGER NOT NULL,
                range_end_nanoseconds INTEGER NOT NULL,
                local_start_julian_day INTEGER NOT NULL,
                local_end_julian_day INTEGER NOT NULL,
                event_id TEXT NOT NULL,
                subject TEXT,
                location TEXT,
                is_all_day INTEGER NOT NULL CHECK (is_all_day IN (0, 1)),
                start_seconds INTEGER,
                start_nanoseconds INTEGER,
                end_seconds INTEGER,
                end_nanoseconds INTEGER,
                start_julian_day INTEGER,
                end_julian_day INTEGER,
                original_start_time_zone TEXT,
                original_end_time_zone TEXT,
                PRIMARY KEY (
                    account_id,
                    range_start_seconds,
                    range_start_nanoseconds,
                    range_end_seconds,
                    range_end_nanoseconds,
                    local_start_julian_day,
                    local_end_julian_day,
                    event_id
                ),
                FOREIGN KEY (
                    account_id,
                    range_start_seconds,
                    range_start_nanoseconds,
                    range_end_seconds,
                    range_end_nanoseconds,
                    local_start_julian_day,
                    local_end_julian_day
                ) REFERENCES calendar_view_sync_state (
                    account_id,
                    range_start_seconds,
                    range_start_nanoseconds,
                    range_end_seconds,
                    range_end_nanoseconds,
                    local_start_julian_day,
                    local_end_julian_day
                ) ON DELETE CASCADE,
                CHECK ((start_nanoseconds IS NULL) = (start_seconds IS NULL)),
                CHECK ((end_nanoseconds IS NULL) = (end_seconds IS NULL)),
                CHECK (start_nanoseconds IS NULL
                       OR start_nanoseconds BETWEEN 0 AND 999999999),
                CHECK (end_nanoseconds IS NULL
                       OR end_nanoseconds BETWEEN 0 AND 999999999),
                CHECK (
                    (is_all_day = 0
                     AND start_seconds IS NOT NULL
                     AND end_seconds IS NOT NULL
                     AND start_julian_day IS NULL
                     AND end_julian_day IS NULL)
                    OR
                    (is_all_day = 1
                     AND start_seconds IS NULL
                     AND end_seconds IS NULL
                     AND start_julian_day IS NOT NULL
                     AND end_julian_day IS NOT NULL
                     AND start_julian_day < end_julian_day)
                )
            );",
        ),
        M::up(
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

            UPDATE mail_messages SET full_sync_generation = NULL;",
        ),
        M::up(
            "CREATE TABLE mail_folder_write_sync_requirement (
                account_id TEXT NOT NULL,
                folder_id TEXT NOT NULL,
                PRIMARY KEY (account_id, folder_id),
                FOREIGN KEY (account_id, folder_id)
                    REFERENCES mail_folders (account_id, folder_id)
                    ON DELETE CASCADE
            );",
        ),
        M::up(
            "CREATE TABLE IF NOT EXISTS mail_pending_send (
                account_id TEXT PRIMARY KEY NOT NULL,
                operation_id TEXT NOT NULL UNIQUE,
                recipient TEXT NOT NULL CHECK (length(recipient) > 0),
                subject TEXT NOT NULL CHECK (length(subject) > 0),
                created_seconds INTEGER NOT NULL,
                created_nanoseconds INTEGER NOT NULL
                    CHECK (created_nanoseconds BETWEEN 0 AND 999999999)
            );",
        ),
    ])
}

pub(super) async fn apply(client: &Client) -> CacheResult<()> {
    client
        .conn_mut_and_then(|connection| {
            apply_parallel_v5_compatibility(connection)?;
            migrations()
                .to_latest(connection)
                .map_err(|_| CacheFailure::Unavailable)
        })
        .await
        .map_err(|_| CacheFailure::Unavailable)?;
    Ok(())
}

fn apply_parallel_v5_compatibility(connection: &mut Connection) -> Result<(), CacheFailure> {
    let migration_set = migrations();
    let version = migration_set
        .current_version(connection)
        .map_err(|_| CacheFailure::Unavailable)?;
    if usize::from(version) != 5 {
        return Ok(());
    }

    let has_flag_status = connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM pragma_table_info('mail_messages') WHERE name = 'flag_status'
            )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| CacheFailure::Unavailable)?;
    if has_flag_status {
        return Ok(());
    }

    let transaction = connection
        .transaction()
        .map_err(|_| CacheFailure::Unavailable)?;
    transaction
        .execute_batch(
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

            UPDATE mail_messages SET full_sync_generation = NULL;",
        )
        .map_err(|_| CacheFailure::Unavailable)?;
    transaction.commit().map_err(|_| CacheFailure::Unavailable)
}

#[cfg(test)]
mod tests {
    use async_sqlite::rusqlite::Connection;

    use super::{apply_parallel_v5_compatibility, migrations};

    #[test]
    fn replacement_generation_migration_preserves_an_existing_snapshot() {
        let mut connection = Connection::open_in_memory().expect("in-memory database opens");
        let migrations = migrations();
        migrations
            .to_version(&mut connection, 1)
            .expect("version one schema is created");
        connection
            .execute_batch(
                "INSERT INTO mail_folders (
                    account_id, folder_id, parent_folder_id, display_name
                 ) VALUES ('account', 'inbox', NULL, 'Inbox');
                 INSERT INTO mail_account_sync_scope (
                    account_id, query_version, cutoff_seconds, cutoff_nanoseconds
                 ) VALUES ('account', 1, 10, 0);
                 INSERT INTO mail_sync_state (
                    account_id, folder_id, delta_link, next_link,
                    last_sync_seconds, last_sync_nanoseconds
                 ) VALUES ('account', 'inbox', 'synthetic-delta', NULL, 20, 0);
                 INSERT INTO mail_messages (
                    account_id, folder_id, message_id, subject,
                    from_display_name, from_address, sender_display_name, sender_address,
                    received_seconds, received_nanoseconds, is_read, body_type, body_content
                 ) VALUES (
                    'account', 'inbox', 'message', 'Synthetic subject',
                    NULL, NULL, NULL, NULL, 20, 0, 0, 'text', 'Synthetic body'
                 );",
            )
            .expect("version one snapshot is inserted");

        migrations
            .to_latest(&mut connection)
            .expect("replacement generation migration applies");

        let sync_state = connection
            .query_row(
                "SELECT full_sync_generation, full_sync_active, checkpoint_revision
                 FROM mail_sync_state WHERE account_id = 'account' AND folder_id = 'inbox'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .expect("existing sync checkpoint remains readable");
        let message = connection
            .query_row(
                "SELECT full_sync_generation, subject, body_content
                 FROM mail_messages
                 WHERE account_id = 'account' AND folder_id = 'inbox' AND message_id = 'message'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .expect("existing message and body remain readable");

        assert_eq!(sync_state, (0, 0, 0));
        assert_eq!(
            message,
            (
                None,
                "Synthetic subject".to_owned(),
                "Synthetic body".to_owned()
            )
        );
    }

    #[test]
    fn flag_migration_preserves_cached_mail_and_resets_message_checkpoints() {
        let mut connection = Connection::open_in_memory().expect("in-memory database opens");
        let migrations = migrations();
        migrations
            .to_version(&mut connection, 4)
            .expect("previous schema is created");
        connection
            .execute_batch(
                "INSERT INTO mail_folders (
                    account_id, folder_id, parent_folder_id, display_name
                 ) VALUES ('account', 'inbox', NULL, 'Inbox');
                 INSERT INTO mail_account_sync_scope (
                    account_id, query_version, cutoff_seconds, cutoff_nanoseconds
                 ) VALUES ('account', 1, 10, 123);
                 INSERT INTO mail_sync_state (
                    account_id, folder_id, delta_link, next_link,
                    last_sync_seconds, last_sync_nanoseconds,
                    full_sync_generation, full_sync_active
                 ) VALUES ('account', 'inbox', 'delta', 'next', 20, 456, 3, 1);
                 INSERT INTO mail_messages (
                    account_id, folder_id, message_id, subject,
                    received_seconds, received_nanoseconds, is_read,
                    body_type, body_content, full_sync_generation
                 ) VALUES (
                    'account', 'inbox', 'message', 'Synthetic subject',
                    20, 0, 1, 'text', 'Synthetic body', 3
                 );",
            )
            .expect("synthetic in-progress snapshot is inserted");

        migrations
            .to_latest(&mut connection)
            .expect("follow-up flag and recovery-marker migrations apply");

        let sync = connection
            .query_row(
                "SELECT a.query_version, a.cutoff_seconds, a.cutoff_nanoseconds,
                        s.delta_link, s.next_link, s.last_sync_seconds,
                        s.last_sync_nanoseconds, s.full_sync_generation,
                        s.full_sync_active
                 FROM mail_account_sync_scope a
                 JOIN mail_sync_state s USING (account_id)",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, bool>(8)?,
                    ))
                },
            )
            .expect("migrated sync state remains readable");
        let message = connection
            .query_row(
                "SELECT subject, flag_status, body_type, body_content, full_sync_generation
                 FROM mail_messages WHERE message_id = 'message'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .expect("cached message remains readable");

        assert_eq!(sync, (2, 10, 123, None, None, None, None, 0, false));
        assert_eq!(
            message,
            (
                "Synthetic subject".to_owned(),
                None,
                Some("text".to_owned()),
                Some("Synthetic body".to_owned()),
                None,
            )
        );
    }

    #[test]
    fn pending_send_schema_contains_only_reconciliation_metadata() {
        let mut connection = Connection::open_in_memory().expect("in-memory database opens");
        migrations()
            .to_latest(&mut connection)
            .expect("pending-send migration applies");
        let mut statement = connection
            .prepare("PRAGMA table_info(mail_pending_send)")
            .expect("pending-send schema is inspectable");
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("pending-send columns are read")
            .collect::<Result<Vec<_>, _>>()
            .expect("pending-send column names are valid");

        assert_eq!(
            columns,
            [
                "account_id",
                "operation_id",
                "recipient",
                "subject",
                "created_seconds",
                "created_nanoseconds",
            ]
        );
        assert!(!columns.iter().any(|column| column.contains("body")));
    }

    #[test]
    fn parallel_send_v5_schema_receives_the_missing_flag_migration() {
        let mut connection = Connection::open_in_memory().expect("in-memory database opens");
        let migrations = migrations();
        migrations
            .to_version(&mut connection, 4)
            .expect("common schema reaches version four");
        connection
            .execute_batch(
                "CREATE TABLE mail_pending_send (
                    account_id TEXT PRIMARY KEY NOT NULL,
                    operation_id TEXT NOT NULL UNIQUE,
                    recipient TEXT NOT NULL CHECK (length(recipient) > 0),
                    subject TEXT NOT NULL CHECK (length(subject) > 0),
                    created_seconds INTEGER NOT NULL,
                    created_nanoseconds INTEGER NOT NULL
                        CHECK (created_nanoseconds BETWEEN 0 AND 999999999)
                );
                PRAGMA user_version = 5;",
            )
            .expect("parallel send branch version five is simulated");

        apply_parallel_v5_compatibility(&mut connection)
            .expect("missing flag migration is applied transactionally");
        migrations
            .to_latest(&mut connection)
            .expect("remaining migrations preserve the existing pending-send table");

        let columns = connection
            .prepare("PRAGMA table_info(mail_messages)")
            .expect("message schema is inspectable")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("message columns are read")
            .collect::<Result<Vec<_>, _>>()
            .expect("message column names are valid");
        assert!(columns.iter().any(|column| column == "flag_status"));

        let pending_send_exists = connection
            .query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'mail_pending_send'
                )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("pending-send table remains available");
        assert!(pending_send_exists);
    }
}
