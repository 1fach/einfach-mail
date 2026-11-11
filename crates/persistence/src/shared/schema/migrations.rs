use rusqlite_migration::{M, Migrations};

use super::compatibility::{FLAG_SCHEMA_TRANSITION_SQL, FOLDER_WRITE_SYNC_REQUIREMENT_TABLE_SQL};

pub(super) fn migrations() -> Migrations<'static> {
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
        M::up(FLAG_SCHEMA_TRANSITION_SQL),
        M::up(FOLDER_WRITE_SYNC_REQUIREMENT_TABLE_SQL),
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
        M::up(
            "ALTER TABLE mail_pending_send RENAME TO mail_pending_send_v1;

            CREATE TABLE mail_pending_send (
                account_id TEXT PRIMARY KEY NOT NULL,
                operation_id TEXT NOT NULL UNIQUE,
                target_kind TEXT NOT NULL CHECK (target_kind IN ('new_message', 'reply')),
                recipient TEXT,
                subject TEXT,
                source_folder_id TEXT,
                source_message_id TEXT,
                created_seconds INTEGER NOT NULL,
                created_nanoseconds INTEGER NOT NULL
                    CHECK (created_nanoseconds BETWEEN 0 AND 999999999),
                CHECK (
                    (target_kind = 'new_message'
                     AND recipient IS NOT NULL AND length(recipient) > 0
                     AND subject IS NOT NULL AND length(subject) > 0
                     AND source_folder_id IS NULL AND source_message_id IS NULL)
                    OR
                    (target_kind = 'reply'
                     AND recipient IS NULL
                     AND source_folder_id IS NOT NULL AND length(source_folder_id) > 0
                     AND source_message_id IS NOT NULL AND length(source_message_id) > 0)
                )
            );

            INSERT INTO mail_pending_send (
                account_id, operation_id, target_kind, recipient, subject,
                source_folder_id, source_message_id,
                created_seconds, created_nanoseconds
            )
            SELECT account_id, operation_id, 'new_message', recipient, subject,
                   NULL, NULL, created_seconds, created_nanoseconds
            FROM mail_pending_send_v1;

            DROP TABLE mail_pending_send_v1;",
        ),
        M::up(
            "CREATE TABLE calendar_pending_event_creation (
                account_id TEXT PRIMARY KEY NOT NULL,
                range_start_seconds INTEGER NOT NULL,
                range_start_nanoseconds INTEGER NOT NULL
                    CHECK (range_start_nanoseconds BETWEEN 0 AND 999999999),
                range_end_seconds INTEGER NOT NULL,
                range_end_nanoseconds INTEGER NOT NULL
                    CHECK (range_end_nanoseconds BETWEEN 0 AND 999999999),
                local_start_julian_day INTEGER NOT NULL,
                local_end_julian_day INTEGER NOT NULL,
                creation_id TEXT NOT NULL UNIQUE,
                subject TEXT NOT NULL,
                local_start_seconds INTEGER NOT NULL,
                local_start_nanoseconds INTEGER NOT NULL
                    CHECK (local_start_nanoseconds BETWEEN 0 AND 999999999),
                local_end_seconds INTEGER NOT NULL,
                local_end_nanoseconds INTEGER NOT NULL
                    CHECK (local_end_nanoseconds BETWEEN 0 AND 999999999),
                time_zone TEXT NOT NULL,
                start_seconds INTEGER NOT NULL,
                start_nanoseconds INTEGER NOT NULL
                    CHECK (start_nanoseconds BETWEEN 0 AND 999999999),
                end_seconds INTEGER NOT NULL,
                end_nanoseconds INTEGER NOT NULL
                    CHECK (end_nanoseconds BETWEEN 0 AND 999999999),
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
                CHECK (local_start_julian_day < local_end_julian_day),
                CHECK (
                    local_start_seconds < local_end_seconds
                    OR (local_start_seconds = local_end_seconds
                        AND local_start_nanoseconds < local_end_nanoseconds)
                ),
                CHECK (
                    start_seconds < end_seconds
                    OR (start_seconds = end_seconds
                        AND start_nanoseconds < end_nanoseconds)
                )
            );",
        ),
        M::up(
            "ALTER TABLE calendar_pending_event_creation
                ADD COLUMN resolution TEXT NOT NULL DEFAULT 'unresolved'
                CHECK (resolution IN ('unresolved', 'rejected'));",
        ),
        M::up(
            "ALTER TABLE calendar_pending_event_creation
                ADD COLUMN view_scope TEXT NOT NULL DEFAULT 'legacy_unbounded'
                CHECK (view_scope IN ('bounded', 'legacy_unbounded'));",
        ),
        M::up(
            "ALTER TABLE calendar_view_sync_state
                ADD COLUMN replacement_active INTEGER NOT NULL DEFAULT 0
                CHECK (replacement_active IN (0, 1));

            CREATE TABLE calendar_event_rebuild (
                account_id TEXT NOT NULL,
                range_start_seconds INTEGER NOT NULL,
                range_start_nanoseconds INTEGER NOT NULL,
                range_end_seconds INTEGER NOT NULL,
                range_end_nanoseconds INTEGER NOT NULL,
                local_start_julian_day INTEGER NOT NULL,
                local_end_julian_day INTEGER NOT NULL,
                event_id TEXT NOT NULL,
                is_in_view INTEGER NOT NULL CHECK (is_in_view IN (0, 1)),
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
            );

            CREATE TABLE calendar_event_rebuild_removed (
                account_id TEXT NOT NULL,
                range_start_seconds INTEGER NOT NULL,
                range_start_nanoseconds INTEGER NOT NULL,
                range_end_seconds INTEGER NOT NULL,
                range_end_nanoseconds INTEGER NOT NULL,
                local_start_julian_day INTEGER NOT NULL,
                local_end_julian_day INTEGER NOT NULL,
                event_id TEXT NOT NULL,
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
                ) ON DELETE CASCADE
            );",
        ),
        M::up(
            "ALTER TABLE calendar_view_sync_state
                ADD COLUMN write_sync_required INTEGER NOT NULL DEFAULT 0
                CHECK (write_sync_required IN (0, 1));

            UPDATE calendar_view_sync_state
            SET write_sync_required = 1
            WHERE last_sync_seconds IS NOT NULL;",
        ),
        M::up(
            "CREATE TABLE calendar_view_snapshot_state (
                account_id TEXT NOT NULL,
                range_start_seconds INTEGER NOT NULL,
                range_start_nanoseconds INTEGER NOT NULL
                    CHECK (range_start_nanoseconds BETWEEN 0 AND 999999999),
                range_end_seconds INTEGER NOT NULL,
                range_end_nanoseconds INTEGER NOT NULL
                    CHECK (range_end_nanoseconds BETWEEN 0 AND 999999999),
                local_start_julian_day INTEGER NOT NULL,
                local_end_julian_day INTEGER NOT NULL,
                completed_seconds INTEGER NOT NULL,
                completed_nanoseconds INTEGER NOT NULL
                    CHECK (completed_nanoseconds BETWEEN 0 AND 999999999),
                PRIMARY KEY (
                    account_id,
                    range_start_seconds,
                    range_start_nanoseconds,
                    range_end_seconds,
                    range_end_nanoseconds,
                    local_start_julian_day,
                    local_end_julian_day
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
                ) ON DELETE CASCADE
            );",
        ),
    ])
}
