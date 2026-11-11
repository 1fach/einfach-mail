use super::*;

#[test]
fn sync_activity_matches_mail_status() {
    assert!(SyncStatus::LoadingCache.is_sync_active());
    assert!(SyncStatus::Syncing.is_sync_active());
    assert!(SyncStatus::Rebuilding.is_sync_active());
    assert!(!SyncStatus::Idle.is_sync_active());
    assert!(!SyncStatus::UpToDate.is_sync_active());
    assert!(!SyncStatus::Failed(MailWorkflowError::AccountMismatch).is_sync_active());
}

#[test]
fn active_mail_operation_explains_when_to_retry() {
    assert_eq!(
        mail_error_message(MailWorkflowError::OperationInProgress),
        "Another mail operation is active. Try again shortly."
    );
}

#[test]
fn empty_message_detail_copy_reflects_message_availability() {
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (_, cache, _) = fixture(account);
    let messages = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .messages
        .clone();
    let folder = messages
        .first()
        .expect("fixture contains a synthetic message")
        .folder()
        .clone();

    assert_eq!(
        empty_message_detail_copy(&MessageListState::Ready, Some(&folder), &messages),
        "Select a message to read it"
    );
    assert_eq!(
        empty_message_detail_copy(&MessageListState::Ready, Some(&folder), &[]),
        "No message selected"
    );
    assert_eq!(
        empty_message_detail_copy(&MessageListState::Ready, None, &messages),
        "No message selected"
    );
    assert_eq!(
        empty_message_detail_copy(&MessageListState::Loading, Some(&folder), &messages),
        "No message selected"
    );
    assert_eq!(
        empty_message_detail_copy(
            &MessageListState::Failed(MailWorkflowError::Provider(
                MailOperationError::ServiceUnavailable,
            )),
            Some(&folder),
            &messages,
        ),
        "No message selected"
    );
}

#[test]
fn stale_sync_checkpoint_explains_how_to_resume() {
    assert_eq!(
        mail_error_message(MailWorkflowError::Cache(MailCacheError::SyncStateChanged)),
        "Another sync updated this folder. Choose Sync to continue."
    );
}

#[test]
fn follow_up_flag_action_label_matches_cached_state() {
    let message_id =
        MessageId::try_from("synthetic-message".to_owned()).expect("synthetic message ID is valid");
    let pending = FollowUpFlagUpdateStatus::Pending {
        message_id: message_id.clone(),
        requested: FollowUpFlagStatus::Flagged,
    };
    let idle = FollowUpFlagUpdateStatus::Idle;

    assert_eq!(
        pending.action_label(&message_id, Some(FollowUpFlagStatus::Complete)),
        "Updating…"
    );
    assert_eq!(
        idle.action_label(&message_id, Some(FollowUpFlagStatus::Flagged)),
        "Clear flag"
    );
    assert_eq!(
        idle.action_label(&message_id, Some(FollowUpFlagStatus::Complete)),
        "Reopen flag"
    );
    assert_eq!(
        idle.action_label(&message_id, Some(FollowUpFlagStatus::NotFlagged)),
        "Flag"
    );
    assert_eq!(idle.action_label(&message_id, None), "Flag");
}
