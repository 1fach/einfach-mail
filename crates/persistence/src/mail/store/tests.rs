use std::time::{Duration, SystemTime, UNIX_EPOCH};

use einfach_mail_application::{MailCache, MailCacheError, MailSyncState};
use einfach_mail_domain::{
    FolderReference, FollowUpFlagStatus, MailFolder, MailMessage, Mailbox, MessageBody,
    MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress,
    MessageId, MessageMetadataPatch, MessagePreview, ReadState, UnreadCount,
};
use tempfile::TempDir;

use crate::SqliteAccountCache;
use crate::shared::store::CacheFailure;
use crate::test_support::{
    account_id, archive_folder, complete_patch, completed_message_delta_page, folder, open_cache,
};

mod message_metadata;
mod unread_counts;

fn received_at() -> SystemTime {
    UNIX_EPOCH + Duration::new(1_800_000_000, 123_456_789)
}

fn message(id: &str) -> MessageId {
    MessageId::try_from(id.to_owned()).expect("synthetic message ID is valid")
}

fn upsert(id: &str, patch: MessageMetadataPatch) -> MessageDeltaChange {
    MessageDeltaChange::Upsert {
        id: message(id),
        patch,
    }
}

async fn cache_with_folders(directory: &TempDir, folders: Vec<MailFolder>) -> SqliteAccountCache {
    let account = account_id();
    let cache = open_cache(directory, account.clone()).await;
    cache
        .replace_folders(&account, folders)
        .await
        .expect("synthetic folders are cached");
    cache
}

async fn sync_state(cache: &SqliteAccountCache, folder: &FolderReference) -> MailSyncState {
    cache
        .ensure_sync_state(folder, received_at() - Duration::from_secs(60))
        .await
        .expect("synthetic checkpoint is readable")
}

/// Commits `changes` as a page that leaves the delta round of `folder` unfinished.
async fn commit_unfinished_page(
    cache: &SqliteAccountCache,
    folder: &FolderReference,
    changes: Vec<MessageDeltaChange>,
) {
    let state = sync_state(cache, folder).await;
    let next_link = MessageDeltaLink::from_opaque(format!(
        "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-{}",
        state.checkpoint_revision()
    ));
    let page = MessageDeltaPage::new(changes, MessageDeltaProgress::Next(next_link));
    MailCache::commit_delta_page(cache, folder, &state, page, received_at())
        .await
        .expect("synthetic unfinished page commits");
}

/// Commits `changes` as the final page of a delta round for `folder`.
async fn complete_round(
    cache: &SqliteAccountCache,
    folder: &FolderReference,
    changes: Vec<MessageDeltaChange>,
) {
    let state = sync_state(cache, folder).await;
    let page = completed_message_delta_page(folder.folder_id().as_str(), changes);
    MailCache::commit_delta_page(cache, folder, &state, page, received_at())
        .await
        .expect("synthetic final page commits");
}
