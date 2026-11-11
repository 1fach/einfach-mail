use super::*;

#[gpui_kit::test]
async fn read_only_account_hides_mail_mutations_and_rejects_composer_entry(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (_, cache, _) = fixture(account.clone());
    let profile = mail_profile_with_capabilities(account, MailCapabilities::read_only());
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("read-only mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    assert!(context.debug_bounds("new-mail-message").is_none());
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached read-only message is visible");
    context.simulate_click(message_row.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(
            view.read(app).message_detail_state,
            MessageDetailState::Opened(_)
        )
    })
    .await;
    draw_mail_view(context);

    for selector in [
        "message-actions",
        "message-read-state",
        "message-follow-up-flag",
        "message-move",
        "message-archive",
        "message-reply",
        "message-delete",
    ] {
        assert!(context.debug_bounds(selector).is_none());
    }
    assert!(
        context
            .cx
            .update(|app| view.read(app).profile.capabilities().mail().is_read_only())
    );

    context.update(|window, app| {
        view.update(app, |mail_view, cx| mail_view.open_composer(window, cx));
        assert!(!window.has_active_sheet(app));
    });
    context.update(|window, app| {
        view.update(app, |mail_view, cx| {
            mail_view.open_reply_composer(window, cx)
        });
        assert!(!window.has_active_sheet(app));
    });
}

#[gpui_kit::test]
async fn partial_mail_capabilities_render_and_allow_only_supported_actions(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (_, cache, _) = fixture(account.clone());
    let profile = mail_profile_with_capabilities(
        account,
        MailCapabilities::from_actions([MailAction::SetReadState]),
    );
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("partial-capability mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    assert!(context.debug_bounds("new-mail-message").is_none());
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message is visible");
    context.simulate_click(message_row.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(
            view.read(app).message_detail_state,
            MessageDetailState::Opened(_)
        )
    })
    .await;
    draw_mail_view(context);

    assert!(context.debug_bounds("message-read-state").is_some());
    for selector in [
        "message-follow-up-flag",
        "message-move",
        "message-archive",
        "message-reply",
        "message-delete",
    ] {
        assert!(context.debug_bounds(selector).is_none());
    }
    assert!(context.cx.update(|app| {
        let mail_view = view.read(app);
        let capabilities = mail_view.profile.capabilities();
        let archive_folder = FolderReference::new(
            mail_view.profile.id().clone(),
            FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
        );
        capabilities.mail().supports(MailAction::SetReadState)
            && !capabilities.mail().supports(MailAction::Send)
            && !capabilities.mail().supports(MailAction::Reply)
            && !capabilities.can_review_mail_write_permission()
            && mail_view.selected_follow_up_flag_request().is_none()
            && mail_view
                .prepare_message_move(archive_folder, "Archive".to_owned())
                .is_none()
            && mail_view
                .prepare_well_known_folder_action(WellKnownMailFolder::DeletedItems)
                .is_none()
    }));

    context.update(|window, app| {
        view.update(app, |mail_view, cx| mail_view.open_composer(window, cx));
        assert!(!window.has_active_sheet(app));
    });
    context.update(|window, app| {
        view.update(app, |mail_view, cx| {
            mail_view.open_reply_composer(window, cx)
        });
        assert!(!window.has_active_sheet(app));
    });
}

#[gpui_kit::test]
async fn message_delete_action_moves_cached_message_to_deleted_items(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = message_move_provider(
        &cache,
        MessageMoveActionResponse::Confirm,
        Arc::clone(&calls),
    );
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    let row = context
        .debug_bounds("message-row-message-1")
        .expect("source message is visible in the cached inbox");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-actions")
    })
    .await;
    let delete_action = context
        .debug_bounds("message-delete")
        .expect("opened message exposes its native Delete action");
    context.simulate_click(delete_action.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-delete-completed")
    })
    .await;
    let deleted_reference = FolderReference::new(
        account,
        FolderId::try_from("deleted-items".to_owned())
            .expect("synthetic Deleted Items ID is valid"),
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("message-delete-completed").is_some());
    assert!(context.cx.update(|cx| view.read(cx).messages.is_empty()));
    let deleted_messages = cache
        .list_messages(&deleted_reference)
        .await
        .expect("Deleted Items cache is readable");
    assert_eq!(deleted_messages.len(), 1);
    assert_eq!(deleted_messages[0].id(), &message_id);
    let deleted_message = cache
        .read_message(&deleted_reference, &message_id)
        .await
        .expect("Deleted Items message is readable")
        .expect("message body remains cached after deletion");
    assert_eq!(
        deleted_message
            .1
            .expect("cached body moves with its message")
            .content(),
        "Synthetic offline body"
    );
}

#[gpui_kit::test]
async fn denied_message_delete_shows_permission_recovery_and_keeps_source(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = message_move_provider(
        &cache,
        MessageMoveActionResponse::AccessDenied,
        Arc::clone(&calls),
    );
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");
    let permission_review_requested = Arc::new(AtomicBool::new(false));
    let observed_permission_review = Arc::clone(&permission_review_requested);
    let _subscription = context.cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &MailViewEvent, _| {
            if matches!(event, MailViewEvent::ReviewMailWritePermission) {
                observed_permission_review.store(true, Ordering::Relaxed);
            }
        })
    });

    wait_for_mail_sync_to_finish(context, &view).await;
    let row = context
        .debug_bounds("message-row-message-1")
        .expect("source message is visible in the cached inbox");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-actions")
    })
    .await;
    let delete_action = context
        .debug_bounds("message-delete")
        .expect("opened message exposes its native Delete action");
    context.simulate_click(delete_action.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-delete-error")
    })
    .await;
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("message-delete-error").is_some());
    assert!(
        context
            .debug_bounds("review-permissions-from-delete")
            .is_some()
    );
    let source_folder = FolderReference::new(
        account,
        FolderId::try_from("inbox".to_owned()).expect("synthetic inbox ID is valid"),
    );
    assert!(
        cache
            .read_message(&source_folder, &message_id)
            .await
            .expect("source cache lookup succeeds")
            .is_some()
    );
    let permission_review = context
        .debug_bounds("review-permissions-from-delete")
        .expect("denied Delete offers delegated permission review");
    context.simulate_click(permission_review.center(), Modifiers::default());
    context.run_until_parked();
    assert!(permission_review_requested.load(Ordering::Relaxed));
    assert!(context.cx.update(|cx| {
        matches!(
            &view.read(cx).well_known_folder_action_status,
            WellKnownFolderActionStatus::Failed {
                destination: WellKnownMailFolder::DeletedItems,
                error,
                ..
            } if *error == MailWorkflowError::Provider(MailOperationError::AccessDenied)
        ) && view.read(cx).move_recovery_reason.is_none()
    }));
}

#[gpui_kit::test]
async fn native_archive_action_moves_cached_message_to_archive(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = message_move_provider(
        &cache,
        MessageMoveActionResponse::Confirm,
        Arc::clone(&calls),
    );
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    let row = context
        .debug_bounds("message-row-message-1")
        .expect("source message is visible in the cached inbox");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-actions")
    })
    .await;
    let archive_action = context
        .debug_bounds("message-archive")
        .expect("opened message exposes its native Archive action");
    context.simulate_click(archive_action.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-move-completed")
    })
    .await;
    let archive_reference = FolderReference::new(
        account,
        FolderId::try_from("archive".to_owned()).expect("synthetic Archive ID is valid"),
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("message-move-completed").is_some());
    assert!(context.cx.update(|cx| view.read(cx).messages.is_empty()));
    let archived_message = cache
        .read_message(&archive_reference, &message_id)
        .await
        .expect("Archive cache is readable")
        .expect("message body remains cached after archiving");
    assert_eq!(
        archived_message
            .1
            .expect("cached body moves with the message")
            .content(),
        "Synthetic offline body"
    );
    let open_archive = context
        .debug_bounds("open-moved-message-folder")
        .expect("archive confirmation offers to open Archive");
    context.simulate_click(open_archive.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        let view = view.read(app);
        view.selected_folder.as_ref() == Some(&archive_reference)
            && view
                .messages
                .iter()
                .any(|message| message.id() == &message_id)
    })
    .await;
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        view.selected_folder.as_ref() == Some(&archive_reference)
            && view
                .messages
                .iter()
                .any(|message| message.id() == &message_id)
    }));
}
#[gpui_kit::test]
async fn pending_message_delete_keeps_the_native_action_busy(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = message_move_provider(
        &cache,
        MessageMoveActionResponse::Pending,
        Arc::clone(&calls),
    );
    let workflow = Arc::new(MailWorkflow::new(profile.id().clone(), provider, cache));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    let row = context
        .debug_bounds("message-row-message-1")
        .expect("source message is visible in the cached inbox");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-actions")
    })
    .await;
    let delete_action = context
        .debug_bounds("message-delete")
        .expect("opened message exposes its native Delete action");
    context.simulate_click(delete_action.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-delete-pending")
    })
    .await;
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("message-delete-pending").is_some());
    assert!(context.cx.update(|cx| {
        view.read(cx).well_known_folder_action_in_progress_for(
            &message_id,
            WellKnownMailFolder::DeletedItems,
        )
    }));
}

#[gpui_kit::test]
async fn unknown_message_delete_requires_sync_and_keeps_source_cached(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = message_move_provider(
        &cache,
        MessageMoveActionResponse::Unknown,
        Arc::clone(&calls),
    );
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    let row = context
        .debug_bounds("message-row-message-1")
        .expect("source message is visible in the cached inbox");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-actions")
    })
    .await;
    let delete_action = context
        .debug_bounds("message-delete")
        .expect("opened message exposes its native Delete action");
    context.simulate_click(delete_action.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-move-recovery")
    })
    .await;
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("message-move-recovery").is_some());
    assert!(context.debug_bounds("sync-after-message-move").is_some());
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        matches!(
            view.move_recovery_reason,
            Some(MoveRecoveryReason::OutcomeUnknown)
        ) && matches!(view.message_detail_state, MessageDetailState::Opened(_))
            && !view.message_move_allowed(MailAction::Move)
    }));
    let source = FolderReference::new(
        account,
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    assert!(
        cache
            .read_message(&source, &message_id)
            .await
            .expect("source cache lookup succeeds after unknown outcome")
            .is_some()
    );
}
