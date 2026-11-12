use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::test_support::calendar_view;
use einfach_mail_application::{
    AccountCache, AccountCacheRemovalError, CalendarCache, CalendarCacheError, MailCache,
    MailCacheError,
};
use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventDraft, CalendarOccurrencePatch,
    CalendarOccurrenceTime, CalendarOccurrenceTimePatch, CalendarTimeZoneId, CalendarViewReference,
    EventId, FolderId, FolderReference, FollowUpFlagStatus, MailFolder, MessageBodyType,
    MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageId,
    MessageMetadataPatch, PendingCalendarEventCreation, ReadState,
};
use tempfile::TempDir;
use time::{Date, Month, PrimitiveDateTime, Time};

use super::{
    SqliteAccountCache,
    time::{system_time_from_parts, system_time_parts},
};
const DATABASE_KEY_BYTE: u8 = 0x51;
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

fn account_id(value: &str) -> AccountId {
    AccountId::try_from(value.to_owned()).expect("synthetic account ID is non-empty")
}

fn folder(account_id: &AccountId, value: &str) -> MailFolder {
    MailFolder::new(
        folder_reference(account_id, value),
        None,
        "Synthetic Inbox".to_owned(),
    )
    .expect("synthetic folder is valid")
}

fn folder_reference(account_id: &AccountId, value: &str) -> FolderReference {
    FolderReference::new(
        account_id.clone(),
        FolderId::try_from(value.to_owned()).expect("synthetic folder ID is non-empty"),
    )
}

fn message_id(value: &str) -> MessageId {
    MessageId::try_from(value.to_owned()).expect("synthetic message ID is non-empty")
}

fn calendar_creation(view: CalendarViewReference) -> CalendarEventCreation {
    let date =
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic event date is valid");
    let local_start = PrimitiveDateTime::new(
        date,
        Time::from_hms(9, 30, 0).expect("synthetic local start is valid"),
    );
    let local_end = PrimitiveDateTime::new(
        date,
        Time::from_hms(10, 0, 0).expect("synthetic local end is valid"),
    );
    let draft = CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        local_start,
        local_end,
        CalendarTimeZoneId::try_from("Etc/UTC".to_owned()).expect("synthetic time zone is valid"),
        date.with_hms(9, 30, 0)
            .expect("synthetic UTC start is valid")
            .assume_utc(),
        date.with_hms(10, 0, 0)
            .expect("synthetic UTC end is valid")
            .assume_utc(),
    )
    .expect("synthetic calendar event draft is valid");
    CalendarEventCreation::try_new(view, draft)
        .expect("synthetic event endpoints fit the selected week")
}

fn calendar_event(creation: &CalendarEventCreation) -> einfach_mail_domain::CalendarOccurrence {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        panic!("synthetic calendar creation uses a timed event");
    };
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(creation.draft().subject().to_owned()));
    let mut event_time = CalendarOccurrenceTimePatch::new();
    event_time.set_start(*start);
    event_time.set_end(*end);
    event_time.set_is_all_day(false);
    patch.set_time(event_time);
    patch
        .into_new(
            creation.view().clone(),
            EventId::try_from("synthetic-created-event".to_owned())
                .expect("synthetic event identifier is valid"),
        )
        .expect("synthetic created calendar event is valid")
}

async fn open_test_cache(directory: &TempDir, account_id: AccountId) -> SqliteAccountCache {
    let database_path = directory.path().join("mail.sqlite3");
    SqliteAccountCache::open_with_test_key(database_path, account_id, DATABASE_KEY_BYTE).await
}

fn complete_patch(
    subject: Option<&str>,
    received_at: SystemTime,
    read_state: ReadState,
) -> MessageMetadataPatch {
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(subject.map(str::to_owned));
    patch.set_from(None);
    patch.set_sender(None);
    patch.set_received_at(received_at);
    patch.set_read_state(read_state);
    patch
}

#[test]
fn page_data_and_opaque_cursor_commit_atomically_and_resume_after_reopen() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let cached_folder = folder(&account, "folder-inbox");
        let folder_reference = cached_folder.reference().clone();
        let cutoff = UNIX_EPOCH + Duration::from_secs(100);
        let cache = open_test_cache(&directory, account.clone()).await;
        cache
            .replace_folders(&account, vec![cached_folder.clone()])
            .await
            .expect("synthetic folder is cached");

        let initial_state = cache
            .ensure_sync_state(&folder_reference, cutoff)
            .await
            .expect("initial cutoff and cursor state are saved");
        assert_eq!(initial_state.cutoff(), cutoff);
        let request_cutoff = cutoff - Duration::from_secs(1_000);
        let resumed_state = cache
            .ensure_sync_state(&folder_reference, request_cutoff)
            .await
            .expect("existing cursor state is reused");
        assert_eq!(resumed_state.cutoff(), cutoff);

        let page = synthetic_initial_page(cutoff);
        let link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-next".to_owned(),
        );
        let page = MessageDeltaPage::new(page, MessageDeltaProgress::Next(link.clone()));
        let first_commit = MailCache::commit_delta_page(
            &cache,
            &folder_reference,
            &initial_state,
            page.clone(),
            cutoff,
        )
        .await
        .expect("first page commits with its resumable cursor");
        assert_eq!(
            MailCache::commit_delta_page(&cache, &folder_reference, &initial_state, page, cutoff,)
                .await,
            Err(MailCacheError::SyncStateChanged)
        );
        assert_eq!(first_commit.next_link(), Some(&link));
        assert!(first_commit.delta_link().is_none());
        assert!(first_commit.last_synchronized_at().is_none());
        assert_eq!(first_commit.checkpoint_revision(), 1);
        assert_eq!(
            cache
                .list_messages(&folder_reference)
                .await
                .expect("cached messages are readable")
                .len(),
            1
        );

        let message = message_id("message-at-cutoff");
        assert_eq!(
            cache
                .message_read_state(&folder_reference, &message)
                .await
                .expect("cached state is readable"),
            Some(ReadState::Unread)
        );
        cache
            .set_message_read_state(&folder_reference, &message, ReadState::Read)
            .await
            .expect("confirmed synthetic read state is stored");
        cache
            .set_message_read_state(&folder_reference, &message, ReadState::Read)
            .await
            .expect("repeating the confirmed state remains idempotent");
        assert_eq!(
            cache
                .message_read_state(&folder_reference, &message)
                .await
                .expect("updated state is readable"),
            Some(ReadState::Read)
        );
        let missing_message = message_id("missing-message");
        assert_eq!(
            cache
                .message_read_state(&folder_reference, &missing_message)
                .await
                .expect("missing cached state is represented as absence"),
            None
        );
        assert_eq!(
            cache
                .set_message_read_state(&folder_reference, &missing_message, ReadState::Read,)
                .await,
            Err(MailCacheError::MessageNotCached)
        );
        cache
            .store_message_body(
                &folder_reference,
                &message,
                einfach_mail_domain::MessageBody::new(
                    MessageBodyType::Html,
                    "<p>Synthetic offline body</p>".to_owned(),
                ),
            )
            .await
            .expect("opened synthetic body is saved");
        cache
            .client
            .close()
            .await
            .expect("synthetic cache connection closes");

        let reopened = open_test_cache(&directory, account).await;
        let persisted_state = reopened
            .load_sync_state(&folder_reference)
            .await
            .expect("stored cursor remains readable")
            .expect("folder cursor exists");
        assert_eq!(persisted_state.next_link(), Some(&link));
        assert_eq!(persisted_state.checkpoint_revision(), 1);
        let (stored_message, stored_body) = reopened
            .read_message(&folder_reference, &message)
            .await
            .expect("cached message is readable after restart")
            .expect("message remains in the folder cache");
        assert_eq!(stored_message.id(), &message);
        assert_eq!(stored_message.metadata().read_state(), ReadState::Read);
        assert_eq!(
            stored_body
                .as_ref()
                .map(einfach_mail_domain::MessageBody::content),
            Some("<p>Synthetic offline body</p>")
        );

        let database_bytes = std::fs::read(&reopened.database_path)
            .expect("synthetic encrypted database is readable as bytes");
        assert_ne!(&database_bytes[..SQLITE_HEADER.len()], SQLITE_HEADER);
        reopened.close_for_tests().await;
    });
}

#[test]
fn confirmed_flag_and_recovery_marker_commit_in_one_transaction() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let cached_folder = folder(&account, "folder-inbox");
        let folder_reference = cached_folder.reference().clone();
        let message = message_id("synthetic-message");
        let cutoff = UNIX_EPOCH + Duration::from_secs(100);
        let cache = open_test_cache(&directory, account.clone()).await;
        cache
            .replace_folders(&account, vec![cached_folder])
            .await
            .expect("synthetic folder is cached");
        let state = cache
            .ensure_sync_state(&folder_reference, cutoff)
            .await
            .expect("initial checkpoint is saved");
        let mut patch = complete_patch(Some("Synthetic message"), cutoff, ReadState::Unread);
        patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::NotFlagged));
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message.clone(),
                patch,
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic".to_owned(),
            )),
        );
        MailCache::commit_delta_page(&cache, &folder_reference, &state, page, cutoff)
            .await
            .expect("synthetic message is cached");
        cache
            .require_folder_write_sync(&folder_reference)
            .await
            .expect("recovery marker is staged");
        cache
            .execute_batch_for_tests(
                "CREATE TRIGGER fail_flag_marker_delete
                 BEFORE DELETE ON mail_folder_write_sync_requirement
                 BEGIN SELECT RAISE(ABORT, 'synthetic marker failure'); END;",
            )
            .await
            .expect("synthetic failure trigger is installed");

        assert_eq!(
            cache
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message,
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailCacheError::Unavailable)
        );
        assert!(
            cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("failed transaction leaves the marker in place")
        );
        assert_eq!(
            cache
                .read_message(&folder_reference, &message)
                .await
                .expect("message is readable after transaction rollback")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::NotFlagged)
        );

        cache
            .execute_batch_for_tests("DROP TRIGGER fail_flag_marker_delete;")
            .await
            .expect("synthetic failure trigger is removed");
        cache
            .set_message_follow_up_flag_status(
                &folder_reference,
                &message,
                FollowUpFlagStatus::Flagged,
            )
            .await
            .expect("confirmed status and marker removal commit together");
        assert!(
            !cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("successful transaction clears the marker")
        );
        assert_eq!(
            cache
                .read_message(&folder_reference, &message)
                .await
                .expect("message is readable after confirmed update")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::Flagged)
        );
        cache.close_for_tests().await;
    });
}

fn synthetic_initial_page(cutoff: SystemTime) -> Vec<MessageDeltaChange> {
    let exact_cutoff = complete_patch(Some("At cutoff"), cutoff, ReadState::Unread);
    let old_message = complete_patch(
        Some("Before cutoff"),
        cutoff - Duration::from_nanos(1),
        ReadState::Read,
    );
    let mut incomplete_patch = MessageMetadataPatch::new();
    incomplete_patch.set_subject(Some("Incomplete metadata".to_owned()));
    vec![
        MessageDeltaChange::Upsert {
            id: message_id("message-at-cutoff"),
            patch: exact_cutoff,
        },
        MessageDeltaChange::Upsert {
            id: message_id("message-before-cutoff"),
            patch: old_message,
        },
        MessageDeltaChange::Upsert {
            id: message_id("message-incomplete"),
            patch: incomplete_patch,
        },
    ]
}

#[test]
fn account_scope_and_folder_snapshot_are_enforced() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let other_account = account_id("other-account");
        let cache = open_test_cache(&directory, account.clone()).await;
        let foreign_folder = folder_reference(&other_account, "folder-inbox");

        assert_eq!(
            cache.list_messages(&foreign_folder).await,
            Err(MailCacheError::AccountMismatch)
        );
        let folders = vec![folder(&account, "folder-inbox")];
        cache
            .replace_folders(&account, folders.clone())
            .await
            .expect("synthetic folder snapshot is valid");
        assert_eq!(
            cache.replace_folders(&other_account, folders).await,
            Err(MailCacheError::AccountMismatch)
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn account_cutoff_survives_an_empty_folder_snapshot_and_later_folders() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let cutoff = UNIX_EPOCH + Duration::from_secs(25_000);
        let cache = open_test_cache(&directory, account.clone()).await;
        cache
            .ensure_account_sync_scope(&account, cutoff)
            .await
            .expect("empty initial account scope is saved");
        cache
            .replace_folders(&account, Vec::new())
            .await
            .expect("empty folder snapshot is valid");
        cache.close_for_tests().await;

        let reopened = open_test_cache(&directory, account.clone()).await;
        let later_folder = folder(&account, "folder-added-later");
        reopened
            .replace_folders(&account, vec![later_folder.clone()])
            .await
            .expect("a later visible folder is saved");
        let later_state = reopened
            .ensure_sync_state(
                later_folder.reference(),
                cutoff + Duration::from_secs(30 * 24 * 60 * 60),
            )
            .await
            .expect("a later folder gets a cursor under the account's original scope");

        assert_eq!(later_state.cutoff(), cutoff);
        reopened.close_for_tests().await;
    });
}

#[test]
fn system_time_parts_round_trip_around_the_unix_epoch() {
    for timestamp in [
        UNIX_EPOCH - Duration::from_nanos(1),
        UNIX_EPOCH,
        UNIX_EPOCH + Duration::from_secs(12) + Duration::from_nanos(345),
    ] {
        let parts = system_time_parts(timestamp).expect("synthetic time fits SQLite");
        assert_eq!(
            system_time_from_parts(parts.0, parts.1).expect("stored synthetic time is valid"),
            timestamp
        );
    }
}

#[test]
fn system_time_from_parts_rejects_invalid_nanoseconds() {
    assert!(system_time_from_parts(0, -1).is_err());
    assert!(system_time_from_parts(0, 1_000_000_000).is_err());
}

#[test]
fn pending_calendar_event_survives_restart_blocks_removal_and_confirms_atomically() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let view = calendar_view(&account);
        let creation = calendar_creation(view.clone());
        let cache = open_test_cache(&directory, account.clone()).await;

        cache
            .stage_event_creation(&creation)
            .await
            .expect("synthetic event operation is staged");
        cache
            .stage_event_creation(&creation)
            .await
            .expect("repeating the same staging operation is idempotent");
        let other_creation = calendar_creation(view.clone());
        assert_eq!(
            cache.stage_event_creation(&other_creation).await,
            Err(CalendarCacheError::CreationAlreadyPending)
        );
        assert_eq!(
            cache.pending_event_creation(&account).await,
            Ok(Some(PendingCalendarEventCreation::Unresolved(
                creation.clone()
            )))
        );
        cache.close_for_tests().await;
        drop(cache);

        let reopened = open_test_cache(&directory, account.clone()).await;
        assert_eq!(
            reopened.pending_event_creation(&account).await,
            Ok(Some(PendingCalendarEventCreation::Unresolved(
                creation.clone()
            )))
        );
        assert_eq!(
            AccountCache::remove_account(&reopened, &account).await,
            Err(AccountCacheRemovalError::PendingCalendarEventCreation)
        );
        assert!(directory.path().join("mail.sqlite3").exists());
        let other_account = account_id("synthetic-other-account");
        assert_eq!(
            reopened.pending_event_creation(&other_account).await,
            Err(CalendarCacheError::AccountMismatch)
        );
        let other_creation = calendar_creation(calendar_view(&other_account));
        assert_eq!(
            reopened.stage_event_creation(&other_creation).await,
            Err(CalendarCacheError::AccountMismatch)
        );
        let event = calendar_event(&creation);
        let mut mismatched_patch = CalendarOccurrencePatch::new();
        mismatched_patch.set_subject(Some("Different synthetic event".to_owned()));
        let CalendarOccurrenceTime::Timed { start, end } = event.time() else {
            panic!("synthetic confirmed calendar event is timed");
        };
        let mut mismatched_time = CalendarOccurrenceTimePatch::new();
        mismatched_time.set_start(*start);
        mismatched_time.set_end(*end);
        mismatched_time.set_is_all_day(false);
        mismatched_patch.set_time(mismatched_time);
        let mismatched_event = mismatched_patch
            .into_new(
                view.clone(),
                EventId::try_from("synthetic-different-event".to_owned())
                    .expect("synthetic event identifier is valid"),
            )
            .expect("synthetic mismatched event has a valid time");
        assert_eq!(
            reopened
                .commit_event_creation(&creation, &mismatched_event)
                .await,
            Err(CalendarCacheError::InvalidData)
        );
        assert_eq!(
            reopened.pending_event_creation(&account).await,
            Ok(Some(PendingCalendarEventCreation::Unresolved(
                creation.clone()
            )))
        );
        assert!(
            reopened
                .list_events(&view)
                .await
                .expect("synthetic event list is readable")
                .is_empty()
        );

        reopened
            .commit_event_creation(&creation, &event)
            .await
            .expect("Graph-confirmed synthetic event commits");
        assert_eq!(reopened.pending_event_creation(&account).await, Ok(None));
        assert_eq!(
            reopened
                .list_events(&view)
                .await
                .expect("confirmed synthetic event is cached")
                .as_slice(),
            std::slice::from_ref(&event)
        );
        reopened.close_for_tests().await;
    });
}

#[test]
fn rejected_calendar_event_survives_restart_and_can_be_discarded_locally() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let creation = calendar_creation(calendar_view(&account));
        let cache = open_test_cache(&directory, account.clone()).await;
        cache
            .stage_event_creation(&creation)
            .await
            .expect("synthetic event operation is staged");

        assert_eq!(
            cache.discard_event_creation(&creation).await,
            Err(CalendarCacheError::CreationAlreadyPending)
        );
        assert_eq!(
            cache.pending_event_creation(&account).await,
            Ok(Some(PendingCalendarEventCreation::Unresolved(
                creation.clone()
            )))
        );

        cache
            .mark_event_creation_rejected(&creation)
            .await
            .expect("the synthetic Graph rejection is persisted");
        cache.close_for_tests().await;

        let reopened = open_test_cache(&directory, account.clone()).await;
        assert_eq!(
            reopened.pending_event_creation(&account).await,
            Ok(Some(PendingCalendarEventCreation::Rejected(
                creation.clone()
            )))
        );
        assert!(
            !reopened
                .has_unresolved_event_creation(&account)
                .await
                .expect("rejected operations do not block account removal")
        );

        reopened
            .discard_event_creation(&creation)
            .await
            .expect("rejected operation can be cleared without another Graph request");
        assert_eq!(reopened.pending_event_creation(&account).await, Ok(None));
        reopened.close_for_tests().await;
    });
}
