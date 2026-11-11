use super::*;

#[gpui_kit::test]
async fn confirmed_follow_up_flag_updates_the_view_and_encrypted_cache(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let folder = FolderReference::new(
        account.clone(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    let (profile, cache, message_id) = fixture(account);
    let updates = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Confirm,
            Arc::new(Mutex::new(Vec::new())),
        )
        .with_flag_updates(Arc::clone(&updates)),
        cache.clone(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    open_cached_message_and_click_follow_up_flag(context);

    assert_eq!(
        context.cx.update(|cx| {
            view.read(cx)
                .messages
                .iter()
                .find(|message| message.id() == &message_id)
                .and_then(|message| message.metadata().follow_up_flag_status())
        }),
        Some(FollowUpFlagStatus::Flagged)
    );
    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).follow_up_flag_update,
        FollowUpFlagUpdateStatus::Confirmed {
            status: FollowUpFlagStatus::Flagged,
            ..
        }
    )));
    assert_eq!(
        cache
            .read_message(&folder, &message_id)
            .await
            .expect("confirmed flag remains readable from the cache")
            .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
        Some(FollowUpFlagStatus::Flagged)
    );
    assert_eq!(
        updates
            .lock()
            .expect("fixture flag-update log mutex is healthy")
            .as_slice(),
        [(message_id, FollowUpFlagStatus::Flagged)]
    );
    assert!(
        context
            .debug_bounds("message-follow-up-flag-feedback")
            .is_some()
    );
}

#[gpui_kit::test]
fn uncertain_follow_up_flag_blocks_a_second_write(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let updates = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Unconfirmed,
            Arc::new(Mutex::new(Vec::new())),
        )
        .with_flag_updates(Arc::clone(&updates)),
        cache.clone(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    open_cached_message_and_click_follow_up_flag(context);

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).follow_up_flag_update,
        FollowUpFlagUpdateStatus::Failed {
            error: MailWorkflowError::Provider(MailOperationError::MessageUpdateOutcomeUnknown),
            ..
        }
    )));
    assert!(
        context
            .cx
            .update(|cx| { view.read(cx).selected_follow_up_flag_request().is_none() })
    );
    assert!(
        context
            .debug_bounds("message-follow-up-flag-feedback")
            .is_some()
    );
    assert!(context.debug_bounds("read-state-reconciliation").is_some());

    let action_bounds = context
        .debug_bounds("message-follow-up-flag")
        .expect("follow-up action remains visible");
    context.simulate_click(action_bounds.center(), Modifiers::default());
    context.run_until_parked();
    assert_eq!(
        updates
            .lock()
            .expect("fixture flag-update log mutex is healthy")
            .as_slice(),
        [(message_id, FollowUpFlagStatus::Flagged)]
    );
    assert!(
        cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .folder_write_sync_required
    );
}

#[test]
fn uncertain_follow_up_flag_failure_explains_sync_recovery() {
    assert_eq!(
        follow_up_flag_failure_message(
            FollowUpFlagStatus::Flagged,
            MailWorkflowError::Provider(MailOperationError::MessageUpdateOutcomeUnknown),
        ),
        "Microsoft may have changed this message's follow-up flag, but Einfach Mail could not confirm it. Sync this folder before trying again."
    );
}
