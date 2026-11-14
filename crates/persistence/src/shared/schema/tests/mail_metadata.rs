use async_sqlite::rusqlite::{Connection, params};

use super::super::migrations;

fn connection_with_one_cached_folder() -> Connection {
    let mut connection = Connection::open_in_memory().expect("in-memory database opens");
    migrations()
        .to_latest(&mut connection)
        .expect("latest schema is created");
    connection
        .execute_batch(
            "INSERT INTO mail_folders (account_id, folder_id, display_name)
             VALUES ('account', 'folder-1', 'Synthetic folder');",
        )
        .expect("synthetic folder is inserted");
    connection
}

fn store_preview(connection: &Connection, message_id: &str, preview: &str) -> bool {
    connection
        .execute(
            "INSERT INTO mail_messages (
                account_id, folder_id, message_id, received_seconds, received_nanoseconds,
                is_read, preview
             ) VALUES ('account', 'folder-1', ?1, 20, 0, 0, ?2)",
            params![message_id, preview],
        )
        .is_ok()
}

fn store_folder_kind(connection: &Connection, folder_id: &str, kind: &str) -> bool {
    connection
        .execute(
            "INSERT INTO mail_folders (account_id, folder_id, display_name, kind)
             VALUES ('account', ?1, 'Synthetic folder', ?2)",
            params![folder_id, kind],
        )
        .is_ok()
}

#[test]
fn mail_metadata_migration_preserves_cached_mail_and_resets_message_checkpoints() {
    let mut connection = Connection::open_in_memory().expect("in-memory database opens");
    let migrations = migrations();
    migrations
        .to_version(&mut connection, 14)
        .expect("previous schema is created");
    connection
        .execute_batch(
            "INSERT INTO mail_folders (
                    account_id, folder_id, parent_folder_id, display_name
                 ) VALUES ('account', 'inbox', NULL, 'Inbox');
                 INSERT INTO mail_account_sync_scope (
                    account_id, query_version, cutoff_seconds, cutoff_nanoseconds
                 ) VALUES ('account', 2, 10, 123);
                 INSERT INTO mail_sync_state (
                    account_id, folder_id, delta_link, next_link,
                    last_sync_seconds, last_sync_nanoseconds,
                    full_sync_generation, full_sync_active, checkpoint_revision
                 ) VALUES ('account', 'inbox', 'delta', 'next', 20, 456, 3, 1, 7);
                 INSERT INTO mail_messages (
                    account_id, folder_id, message_id, subject,
                    received_seconds, received_nanoseconds, is_read, flag_status,
                    body_type, body_content, full_sync_generation
                 ) VALUES (
                    'account', 'inbox', 'message', 'Synthetic subject',
                    20, 0, 0, 'flagged', 'text', 'Synthetic body', 3
                 );",
        )
        .expect("synthetic in-progress snapshot is inserted");

    migrations
        .to_latest(&mut connection)
        .expect("mail metadata migration applies");

    let folder = connection
        .query_row(
            "SELECT display_name, kind, unread_reported, unread_uncached
                 FROM mail_folders WHERE account_id = 'account' AND folder_id = 'inbox'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .expect("cached folder remains readable");
    let sync = connection
        .query_row(
            "SELECT a.query_version, a.cutoff_seconds, a.cutoff_nanoseconds,
                        s.delta_link, s.next_link, s.last_sync_seconds,
                        s.last_sync_nanoseconds, s.full_sync_generation,
                        s.full_sync_active, s.checkpoint_revision
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
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .expect("migrated sync state remains readable");
    let message = connection
        .query_row(
            "SELECT subject, is_read, flag_status, body_type, body_content,
                        full_sync_generation, preview
                 FROM mail_messages WHERE message_id = 'message'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .expect("cached message remains readable");

    assert_eq!(folder, ("Inbox".to_owned(), None, None, None));
    assert_eq!(sync, (3, 10, 123, None, None, None, None, 0, false, 8));
    assert_eq!(
        message,
        (
            "Synthetic subject".to_owned(),
            false,
            Some("flagged".to_owned()),
            Some("text".to_owned()),
            Some("Synthetic body".to_owned()),
            None,
            None,
        )
    );
}

#[test]
fn message_preview_holds_between_1_and_256_characters() {
    let connection = connection_with_one_cached_folder();

    assert!(store_preview(&connection, "longest", &"ä".repeat(256)));
    assert!(!store_preview(&connection, "too-long", &"ä".repeat(257)));
    assert!(!store_preview(&connection, "empty", ""));
}

#[test]
fn folder_kind_accepts_only_the_known_names() {
    let connection = connection_with_one_cached_folder();

    for (index, kind) in [
        "inbox",
        "sent_items",
        "drafts",
        "archive",
        "deleted_items",
        "junk_email",
        "other",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            store_folder_kind(&connection, &format!("known-{index}"), kind),
            "{kind} is a known folder kind"
        );
    }
    assert!(!store_folder_kind(&connection, "unknown", "outbox"));
}

#[test]
fn folder_unread_counts_cannot_be_negative() {
    let connection = connection_with_one_cached_folder();
    let store_count = |column: &str, count: i64| {
        connection
            .execute(
                &format!(
                    "UPDATE mail_folders SET {column} = ?1
                     WHERE account_id = 'account' AND folder_id = 'folder-1'"
                ),
                [count],
            )
            .is_ok()
    };

    for column in ["unread_reported", "unread_uncached"] {
        assert!(store_count(column, 0), "{column} accepts zero");
        assert!(
            !store_count(column, -1),
            "{column} rejects a negative count"
        );
    }
}

#[test]
fn an_account_holds_one_folder_of_each_kind_and_many_plain_folders() {
    let connection = connection_with_one_cached_folder();

    assert!(store_folder_kind(&connection, "inbox-1", "inbox"));
    assert!(!store_folder_kind(&connection, "inbox-2", "inbox"));
    assert!(store_folder_kind(&connection, "plain-1", "other"));
    assert!(store_folder_kind(&connection, "plain-2", "other"));
    connection
        .execute_batch(
            "INSERT INTO mail_folders (account_id, folder_id, display_name, kind)
             VALUES ('other-account', 'inbox-1', 'Synthetic folder', 'inbox');",
        )
        .expect("another account has its own Inbox");
}
