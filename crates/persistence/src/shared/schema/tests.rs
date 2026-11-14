use async_sqlite::rusqlite::{Connection, params};

use super::{MESSAGE_QUERY_VERSION, apply_parallel_schema_compatibility, migrations};

mod mail_metadata;

#[test]
fn bundled_sqlcipher_source_matches_reviewed_versions() {
    let connection = Connection::open_in_memory().expect("in-memory database opens");
    let cipher_version: String = connection
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .expect("SQLCipher runtime version is readable");
    let sqlite_version: String = connection
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .expect("SQLite runtime version is readable");

    assert_eq!(cipher_version.split_whitespace().next(), Some("4.20.0"));
    assert_eq!(sqlite_version, "3.54.1");
}

fn assert_flag_transition_preserves_cached_mail(connection: &Connection) {
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

    assert_eq!(
        sync,
        (
            MESSAGE_QUERY_VERSION,
            10,
            123,
            None,
            None,
            None,
            None,
            0,
            false
        )
    );
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

    assert_eq!(
        sync_state,
        (0, 0, 1),
        "the mail metadata migration advances the checkpoint revision once"
    );
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
    assert_flag_transition_preserves_cached_mail(&connection);
}

fn mail_metadata_columns_exist(connection: &Connection) -> bool {
    connection
        .prepare("SELECT preview FROM mail_messages")
        .and_then(|_| {
            connection.prepare("SELECT kind, unread_reported, unread_uncached FROM mail_folders")
        })
        .is_ok()
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
            "target_kind",
            "recipient",
            "subject",
            "source_folder_id",
            "source_message_id",
            "created_seconds",
            "created_nanoseconds",
        ]
    );
    assert!(!columns.iter().any(|column| column.contains("body")));
}

#[test]
fn reply_marker_migration_preserves_existing_new_message_markers() {
    let mut connection = Connection::open_in_memory().expect("in-memory database opens");
    let migrations = migrations();
    migrations
        .to_version(&mut connection, 7)
        .expect("legacy send schema is created");
    connection
        .execute(
            "INSERT INTO mail_pending_send (
                    account_id, operation_id, recipient, subject,
                    created_seconds, created_nanoseconds
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "synthetic-account",
                "8d719f02-64d5-4c4b-905e-0d8ca42c21a8",
                "ada@example.test",
                "Synthetic subject",
                1_800_000_000_i64,
                123_456_789_i64,
            ],
        )
        .expect("legacy pending marker is stored");

    migrations
        .to_latest(&mut connection)
        .expect("tagged marker migration applies");
    let migrated = connection
        .query_row(
            "SELECT target_kind, recipient, subject, source_folder_id,
                        source_message_id, created_seconds, created_nanoseconds
                 FROM mail_pending_send WHERE account_id = ?1",
            ["synthetic-account"],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .expect("migrated pending marker is readable");

    assert_eq!(
        migrated,
        (
            "new_message".to_owned(),
            Some("ada@example.test".to_owned()),
            Some("Synthetic subject".to_owned()),
            None,
            None,
            1_800_000_000,
            123_456_789,
        )
    );
}

#[test]
fn reply_aware_parallel_schema_keeps_unresolved_reply_marker() {
    let mut connection = Connection::open_in_memory().expect("in-memory database opens");
    migrations()
        .to_version(&mut connection, 5)
        .expect("common schema reaches version five");
    connection
        .execute_batch(
            "CREATE TABLE mail_pending_send (
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
                        (target_kind = 'new_message' AND recipient IS NOT NULL
                         AND subject IS NOT NULL AND source_folder_id IS NULL
                         AND source_message_id IS NULL)
                        OR
                        (target_kind = 'reply' AND recipient IS NULL
                         AND source_folder_id IS NOT NULL AND source_message_id IS NOT NULL)
                    )
                );
                INSERT INTO mail_pending_send (
                    account_id, operation_id, target_kind, recipient, subject,
                    source_folder_id, source_message_id,
                    created_seconds, created_nanoseconds
                ) VALUES (
                    'synthetic-account', '8d719f02-64d5-4c4b-905e-0d8ca42c21a8',
                    'reply', NULL, 'Synthetic subject', 'inbox', 'source-message',
                    1_800_000_000, 123456789
                );
                PRAGMA user_version = 7;",
        )
        .expect("reply-aware parallel schema is simulated");

    apply_parallel_schema_compatibility(&mut connection)
        .expect("parallel schema is normalized without rewriting the marker");
    apply_parallel_schema_compatibility(&mut connection)
        .expect("completed parallel schema repair is safe to retry");
    migrations()
        .to_latest(&mut connection)
        .expect("normalized schema is already current");
    let marker = connection
        .query_row(
            "SELECT target_kind, source_folder_id, source_message_id,
                        created_seconds, created_nanoseconds
                 FROM mail_pending_send WHERE account_id = 'synthetic-account'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .expect("unresolved reply marker remains readable");
    let folder_write_table_exists = connection
        .query_row(
            "SELECT EXISTS (
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'mail_folder_write_sync_requirement'
                )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .expect("folder-write marker schema is inspectable");

    assert_eq!(
        marker,
        (
            "reply".to_owned(),
            "inbox".to_owned(),
            "source-message".to_owned(),
            1_800_000_000,
            123_456_789,
        )
    );
    assert!(folder_write_table_exists);
    assert!(mail_metadata_columns_exist(&connection));
}

#[test]
fn reply_schema_repair_preserves_an_existing_folder_write_table() {
    let mut connection = Connection::open_in_memory().expect("in-memory database opens");
    migrations()
        .to_version(&mut connection, 6)
        .expect("folder-write table migration applies");
    connection
        .execute_batch(
            "CREATE TABLE mail_pending_send (
                    account_id TEXT NOT NULL,
                    target_kind TEXT NOT NULL
                );
                PRAGMA user_version = 7;",
        )
        .expect("reply-aware parallel schema is simulated");

    apply_parallel_schema_compatibility(&mut connection)
        .expect("reply repair accepts the table created by its migration");
    apply_parallel_schema_compatibility(&mut connection)
        .expect("completed reply repair is safe to retry");

    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .expect("repaired schema version is readable");
    let folder_write_table_exists = connection
        .query_row(
            "SELECT EXISTS (
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'mail_folder_write_sync_requirement'
                )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .expect("folder-write table remains available");

    assert_eq!(version, 8);
    assert!(folder_write_table_exists);
}

#[test]
fn folder_write_table_migration_rejects_an_existing_table() {
    let mut connection = Connection::open_in_memory().expect("in-memory database opens");
    let migrations = migrations();
    migrations
        .to_version(&mut connection, 6)
        .expect("folder-write table migration applies");
    connection
        .execute_batch("PRAGMA user_version = 5;")
        .expect("pre-migration version is restored");

    assert!(migrations.to_version(&mut connection, 6).is_err());
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
                 );
                 CREATE TABLE mail_pending_send (
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

    apply_parallel_schema_compatibility(&mut connection)
        .expect("missing flag migration is applied transactionally");
    migrations
        .to_latest(&mut connection)
        .expect("remaining migrations preserve the existing pending-send table");
    assert_flag_transition_preserves_cached_mail(&connection);

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
    assert!(mail_metadata_columns_exist(&connection));
}

#[test]
fn event_creation_migrations_preserve_legacy_marker_recovery_policy() {
    let mut connection = Connection::open_in_memory().expect("in-memory database opens");
    migrations()
        .to_version(&mut connection, 9)
        .expect("previous schema is applied");
    connection
        .execute(
            "INSERT INTO calendar_view_sync_state (
                    account_id, range_start_seconds, range_start_nanoseconds,
                    range_end_seconds, range_end_nanoseconds, local_start_julian_day,
                    local_end_julian_day
                ) VALUES (
                    ?1, 1893974400, 0, 1894579200, 0, 2462504, 2462511
                )",
            ["synthetic-account"],
        )
        .expect("legacy calendar view is stored");
    connection
        .execute(
            "INSERT INTO calendar_pending_event_creation (
                    account_id, range_start_seconds, range_start_nanoseconds,
                    range_end_seconds, range_end_nanoseconds, local_start_julian_day,
                    local_end_julian_day, creation_id, subject, local_start_seconds,
                    local_start_nanoseconds, local_end_seconds, local_end_nanoseconds,
                    time_zone, start_seconds, start_nanoseconds, end_seconds,
                    end_nanoseconds
                ) VALUES (
                    ?1, 1893974400, 0, 1894579200, 0, 2462504, 2462511,
                    ?2, 'Synthetic event', 1894006800, 0, 1894010400, 0,
                    'Etc/UTC', 1894006800, 0, 1894010400, 0
                )",
            params!["synthetic-account", "8d719f02-64d5-4c4b-905e-0d8ca42c21a8"],
        )
        .expect("legacy event creation marker is stored");

    migrations()
        .to_latest(&mut connection)
        .expect("event creation resolution migrations apply");
    let marker_metadata = connection
        .query_row(
            "SELECT resolution, view_scope FROM calendar_pending_event_creation
                 WHERE account_id = ?1",
            ["synthetic-account"],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("legacy marker remains recoverable");

    assert_eq!(
        marker_metadata,
        ("unresolved".to_owned(), "legacy_unbounded".to_owned())
    );
}
#[test]
fn calendar_replacement_migration_preserves_existing_events_and_cursor() {
    let mut connection = Connection::open_in_memory().expect("in-memory database opens");
    let migrations = migrations();
    migrations
        .to_version(&mut connection, 4)
        .expect("calendar schema is created");
    connection
        .execute_batch(
            "INSERT INTO calendar_view_sync_state (
                    account_id, range_start_seconds, range_start_nanoseconds,
                    range_end_seconds, range_end_nanoseconds,
                    local_start_julian_day, local_end_julian_day,
                    delta_link, next_link, last_sync_seconds, last_sync_nanoseconds
                 ) VALUES (
                    'account', 100, 0, 200, 0, 2462500, 2462507,
                    'synthetic-delta', NULL, 300, 0
                 );
                 INSERT INTO calendar_view_sync_state (
                    account_id, range_start_seconds, range_start_nanoseconds,
                    range_end_seconds, range_end_nanoseconds,
                    local_start_julian_day, local_end_julian_day
                 ) VALUES (
                    'account', 300, 0, 400, 0, 2462510, 2462517
                 );
                 INSERT INTO calendar_events (
                    account_id, range_start_seconds, range_start_nanoseconds,
                    range_end_seconds, range_end_nanoseconds,
                    local_start_julian_day, local_end_julian_day, event_id,
                    subject, location, is_all_day,
                    start_seconds, start_nanoseconds, end_seconds, end_nanoseconds,
                    start_julian_day, end_julian_day,
                    original_start_time_zone, original_end_time_zone
                 ) VALUES (
                    'account', 100, 0, 200, 0, 2462500, 2462507, 'event',
                    'Synthetic event', NULL, 0, 120, 0, 150, 0,
                    NULL, NULL, NULL, NULL
                 );",
        )
        .expect("existing calendar snapshot is inserted");

    migrations
        .to_latest(&mut connection)
        .expect("calendar replacement migration applies");

    let sync_state = connection
        .query_row(
            "SELECT delta_link, next_link, last_sync_seconds,
                        last_sync_nanoseconds, replacement_active, write_sync_required
                 FROM calendar_view_sync_state
                 WHERE account_id = 'account' AND range_start_seconds = 100",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .expect("existing calendar cursor remains readable");
    let event = connection
        .query_row(
            "SELECT event_id, subject
                 FROM calendar_events WHERE account_id = 'account'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("existing calendar event remains readable");

    assert_eq!(
        sync_state,
        (
            Some("synthetic-delta".to_owned()),
            None,
            Some(300),
            Some(0),
            0,
            1
        )
    );
    let incomplete_view_marker = connection
        .query_row(
            "SELECT write_sync_required FROM calendar_view_sync_state
                 WHERE account_id = 'account' AND range_start_seconds = 300",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("migration leaves an incomplete view unmarked");
    assert_eq!(incomplete_view_marker, 0);
    assert_eq!(event, ("event".to_owned(), "Synthetic event".to_owned()));
}
