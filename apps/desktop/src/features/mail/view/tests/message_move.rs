use super::*;

#[gpui_kit::test]
async fn message_move_menu_moves_cached_message_and_opens_destination(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let destination = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .iter()
        .find(|folder| folder.display_name() == "Archive")
        .expect("fixture cache contains a visible archive folder")
        .reference()
        .clone();
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
    let move_action = context
        .debug_bounds("message-move")
        .expect("opened message exposes its native move menu");
    assert!(move_action.size.width > px(0.));
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.apply_message_move_menu_action(
                MessageMoveMenuAction {
                    selected_message_id: message_id.clone(),
                    destination_folder: destination.clone(),
                    destination_label: "Archive".to_owned(),
                },
                cx,
            );
        });
    });

    wait_for_ui_condition(context, |_, app| {
        matches!(
            view.read(app).message_detail_state,
            MessageDetailState::MoveCompleted { .. }
        )
    })
    .await;
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("message-move-completed").is_some());
    assert!(context.cx.update(|cx| view.read(cx).messages.is_empty()));
    let archived_messages = cache
        .list_messages(&destination)
        .await
        .expect("moved message list is readable");
    assert_eq!(archived_messages.len(), 1);
    assert_eq!(archived_messages[0].id(), &message_id);

    let open_folder = context
        .debug_bounds("open-moved-message-folder")
        .expect("move confirmation offers the destination folder");
    context.simulate_click(open_folder.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        let view = view.read(app);
        view.selected_folder.as_ref() == Some(&destination)
            && view
                .messages
                .iter()
                .any(|message| message.id() == &message_id)
    })
    .await;
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        view.selected_folder.as_ref() == Some(&destination)
            && view
                .messages
                .iter()
                .any(|message| message.id() == &message_id)
    }));
}

#[gpui_kit::test]
async fn folder_write_sync_requirement_disables_and_blocks_message_move(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account.clone());
    let destination = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .iter()
        .find(|folder| folder.display_name() == "Archive")
        .expect("fixture cache contains a visible archive folder")
        .reference()
        .clone();
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

    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.selected_folder_write_sync_required = true;
            cx.notify();
        });
    });
    draw_mail_view(context);
    let move_action = context
        .debug_bounds("message-move")
        .expect("opened message still exposes its disabled move action");
    context.simulate_click(move_action.center(), Modifiers::default());
    context.run_until_parked();
    assert!(context.debug_bounds("read-state-reconciliation").is_some());
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        !view.message_move_allowed(MailAction::Move)
            && view
                .prepare_message_move(destination.clone(), "Archive".to_owned())
                .is_none()
    }));
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.apply_message_move_menu_action(
                MessageMoveMenuAction {
                    selected_message_id: message_id,
                    destination_folder: destination,
                    destination_label: "Archive".to_owned(),
                },
                cx,
            );
        });
    });

    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[gpui_kit::test]
async fn unknown_message_move_shows_sync_recovery_and_keeps_source_cached(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account.clone());
    let destination = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .iter()
        .find(|folder| folder.display_name() == "Archive")
        .expect("fixture cache contains a visible archive folder")
        .reference()
        .clone();
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
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.move_selected_message_to(destination.clone(), "Archive".to_owned(), cx);
        });
    });

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-move-recovery")
    })
    .await;
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("message-move-recovery").is_some());
    assert!(context.debug_bounds("sync-after-message-move").is_some());
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        view.move_recovery_reason.is_some()
            && matches!(view.message_detail_state, MessageDetailState::Opened(_))
    }));
    let source = FolderReference::new(
        account,
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    assert!(
        cache
            .read_message(&source, &message_id)
            .await
            .expect("source cache lookup succeeds")
            .is_some()
    );
}

#[gpui_kit::test]
async fn confirmed_message_move_with_cache_failure_requests_sync(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account.clone());
    let destination = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .iter()
        .find(|folder| folder.display_name() == "Archive")
        .expect("fixture cache contains a visible archive folder")
        .reference()
        .clone();
    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .move_error = Some(MailCacheError::Unavailable);
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
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.move_selected_message_to(destination.clone(), "Archive".to_owned(), cx);
        });
    });

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-move-recovery")
    })
    .await;
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("message-move-recovery").is_some());
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        matches!(view.message_detail_state, MessageDetailState::Empty)
            && matches!(
                view.move_recovery_reason,
                Some(MoveRecoveryReason::CacheNeedsSync)
            )
    }));
    let source = FolderReference::new(
        account,
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    assert!(
        cache
            .read_message(&source, &message_id)
            .await
            .expect("source cache lookup succeeds")
            .is_some()
    );
}
