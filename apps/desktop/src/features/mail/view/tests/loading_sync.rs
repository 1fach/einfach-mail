use super::*;

#[gpui_kit::test]
async fn mail_sync_retry_stays_visible_at_wide_width(cx: &mut TestAppContext) {
    assert_mail_sync_retry_at_width(cx, 1280.).await;
}

#[gpui_kit::test]
async fn mail_sync_retry_stays_visible_at_compact_width(cx: &mut TestAppContext) {
    assert_mail_sync_retry_at_width(cx, 640.).await;
}

async fn assert_mail_sync_retry_at_width(cx: &mut TestAppContext, width: f32) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Confirm,
            Arc::new(Mutex::new(Vec::new())),
        )
        .with_sync_failure_once(),
        cache,
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let mail_view = mail_view.expect("mail view is attached to folder navigation");
    context.simulate_resize(size(px(width), px(800.)));

    wait_for_mail_sync_to_finish(context, &mail_view).await;
    assert_mail_sync_failure_is_accessible(context, &mail_view, width);

    context.update(|window, cx| window.click("sync-mail", cx));
    wait_for_mail_sync_to_finish(context, &mail_view).await;
    assert_mail_sync_success_is_accessible(context);
}

fn assert_mail_sync_failure_is_accessible(
    context: &mut VisualTestContext,
    mail_view: &Entity<MailView>,
    width: f32,
) {
    let failed_status = context
        .cx
        .update(|app| mail_view.read(app).sync_status_label());
    let (failure_role, failure_label, failure_visible, retry_visible) =
        context.update(|window, _| {
            let failure = window.find("mail-sync-status");
            let retry = window.find("sync-mail");
            (
                failure.role(),
                failure.label().map(str::to_owned),
                failure.visible(),
                retry.visible(),
            )
        });
    assert_eq!(failure_role, Some(gpui_kit::Role::Alert));
    assert_eq!(failure_label.as_deref(), Some(failed_status.as_str()));
    assert!(failure_visible, "Mail sync failure must remain visible");
    assert!(
        retry_visible,
        "Sync action must remain visible at {width}px"
    );
}

fn assert_mail_sync_success_is_accessible(context: &mut VisualTestContext) {
    let (success_role, success_label) = context.update(|window, _| {
        let status = window.find("mail-sync-status");
        (status.role(), status.label().map(str::to_owned))
    });
    assert_eq!(success_role, Some(gpui_kit::Role::Status));
    assert!(
        success_label
            .as_deref()
            .is_some_and(|label| label.starts_with("Up to date")),
        "successful retry reports an updated mail state: {success_label:?}"
    );
}

#[gpui_kit::test]
async fn message_body_failure_keeps_the_cached_list_and_selection(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    cache.0.lock().expect("fixture cache mutex is healthy").body = None;
    let provider = OfflineProvider {
        body_fetches: Arc::new(AtomicUsize::new(0)),
    };
    let body_fetches = Arc::clone(&provider.body_fetches);
    let workflow = Arc::new(MailWorkflow::new(profile.id().clone(), provider, cache));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    wait_for_mail_sync_to_finish(context, &view).await;

    let row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is visible before opening");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(
            &view.read(app).message_detail_state,
            MessageDetailState::Failed { .. }
        )
    })
    .await;

    assert!(context.debug_bounds("message-row-message-1").is_some());
    assert_eq!(
        accessible_alert_count(
            context,
            mail_error_message(MailWorkflowError::Provider(
                MailOperationError::ServiceUnavailable
            ))
        ),
        1,
        "the body-fetch failure appears once in the accessible alerts"
    );
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        matches!(&view.message_list_state, MessageListState::Ready)
            && matches!(
                &view.message_detail_state,
                MessageDetailState::Failed {
                    error: MailWorkflowError::Provider(_),
                    ..
                }
            )
            && view.selected_message_id() == Some(&message_id)
    }));
    assert_selected_message_and_accessible_label(
        context,
        &view,
        Some(&message_id),
        "Ada Example <ada@example.test>. Synthetic subject",
    );
    assert_eq!(body_fetches.load(Ordering::Relaxed), 1);
}

#[gpui_kit::test]
async fn folder_message_list_failure_does_not_claim_a_message_is_loading(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");

    wait_for_mail_sync_to_finish(context, &view).await;

    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .list_messages_error = Some(MailCacheError::Unavailable);
    let archive = context.update(|window, _| window.find_by_label("Archive"));
    context.simulate_click(archive.bounds().center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(
            &view.read(app).message_list_state,
            MessageListState::Failed(_)
        )
    })
    .await;

    assert_eq!(
        accessible_alert_count(
            context,
            mail_error_message(MailWorkflowError::Cache(MailCacheError::Unavailable))
        ),
        1,
        "the folder-list failure appears once in the accessible alerts"
    );
    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        matches!(
            &view.message_list_state,
            MessageListState::Failed(MailWorkflowError::Cache(MailCacheError::Unavailable))
        ) && matches!(&view.message_detail_state, MessageDetailState::Empty)
    }));
}

#[gpui_kit::test]
fn offline_mail_view_opens_cached_message_without_provider_body_request(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let provider = OfflineProvider {
        body_fetches: Arc::new(AtomicUsize::new(0)),
    };
    let body_fetches = Arc::clone(&provider.body_fetches);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    context.run_until_parked();
    draw_mail_view(context);
    let row_bounds = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(row_bounds.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    let opened_offline = context.cx.update(|cx| {
        matches!(
            &view.read(cx).message_detail_state,
            MessageDetailState::Opened(opened)
                if opened.message().id() == &message_id
                    && opened.loaded_from_cache()
                    && matches!(
                        opened.body(),
                        MessageBodyForDisplay::PlainText(text)
                            if text.as_ref() == "Synthetic offline body"
                    )
        )
    });
    assert!(opened_offline);
    assert!(context.cx.update(|cx| {
        matches!(
            view.read(cx).move_recovery_reason,
            Some(MoveRecoveryReason::InitialSyncRequired)
        )
    }));
    assert_eq!(body_fetches.load(Ordering::Relaxed), 0);
}

#[gpui_kit::test]
async fn cached_folders_and_open_message_remain_available_during_sync(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_message_navigation_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, inbox_message_id) = fixture(account.clone());
    let archive_reference = FolderReference::new(
        account,
        FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
    );
    let archive_message_id =
        MessageId::try_from("archive-message".to_owned()).expect("synthetic message ID is valid");
    {
        let mut state = cache.0.lock().expect("fixture cache mutex is healthy");
        let sender = Mailbox::try_new(
            "ada@example.test".to_owned(),
            Some("Ada Example".to_owned()),
        )
        .expect("synthetic mailbox is valid");
        state.messages.push(MailMessage::new(
            archive_reference.clone(),
            archive_message_id.clone(),
            einfach_mail_domain::MessageMetadata::new(
                Some("Archived synthetic subject".to_owned()),
                einfach_mail_domain::MessageOrigin::new(Some(sender), None),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                ReadState::Unread,
                None,
            ),
        ));
    }
    let folders = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .clone();
    let inbox_reference = folders
        .iter()
        .find(|folder| folder.display_name() == "Inbox")
        .expect("fixture contains the inbox folder")
        .reference()
        .clone();
    let (sync_started, sync_started_receiver) = tokio::sync::oneshot::channel();
    let (resume_sync, sync_release) = tokio::sync::oneshot::channel();
    let provider = ReadStateProvider::new(
        ReadStateActionResponse::Confirm,
        Arc::new(Mutex::new(Vec::new())),
    )
    .with_folders(folders.clone())
    .hold_sync_until_released(sync_started, sync_release);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");

    sync_started_receiver
        .await
        .expect("account sync reaches its provider gate after loading the cache");
    assert!(context.cx.update(|app| {
        matches!(view.read(app).sync_status, SyncStatus::Syncing)
            && view.read(app).selected_folder.is_some()
    }));

    let archive_button = context.update(|window, _| window.find_by_label("Archive"));
    context.simulate_click(archive_button.bounds().center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        let view = view.read(app);
        view.selected_folder.as_ref() == Some(&archive_reference)
            && matches!(view.message_list_state, MessageListState::Ready)
    })
    .await;

    context.update(gpui_kit::Window::focus_next);
    let mut list_focused = false;
    for _ in 0..20 {
        press_window_key(context, "tab");
        list_focused =
            context.update(|window, app| view.read(app).message_list_focus.is_focused(window));
        if list_focused {
            break;
        }
    }
    assert!(
        list_focused,
        "Tab reaches cached folders' message list during sync"
    );
    press_window_key(context, "home");
    wait_for_ui_condition(context, |_, app| {
        matches!(
            view.read(app).selected_message_id(),
            Some(message_id) if message_id == &archive_message_id
        ) && matches!(
            &view.read(app).message_detail_state,
            MessageDetailState::Opened(opened) if opened.loaded_from_cache()
        )
    })
    .await;

    let (snapshot_list_started, snapshot_list_release) = cache.hold_next_folder_list();
    resume_sync
        .send(())
        .expect("the test releases the held mail sync");
    snapshot_list_started
        .await
        .expect("sync completion starts loading its selected-folder snapshot");

    let inbox_button = context.update(|window, _| window.find_by_label("Inbox"));
    context.simulate_click(inbox_button.bounds().center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        let view = view.read(app);
        view.selected_folder.as_ref() == Some(&inbox_reference)
            && matches!(view.message_list_state, MessageListState::Ready)
    })
    .await;
    let inbox_row = context
        .debug_bounds("message-row-message-1")
        .expect("the cached inbox message is visible");
    context.simulate_click(inbox_row.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, app| {
        matches!(
            &view.read(app).message_detail_state,
            MessageDetailState::Opened(opened)
                if opened.message().id() == &inbox_message_id
                    && opened.loaded_from_cache()
        )
    })
    .await;
    snapshot_list_release
        .send(())
        .expect("the test releases the selected-folder snapshot");

    wait_for_mail_sync_to_finish(context, &view).await;
    wait_for_ui_condition(context, |_, app| {
        let view = view.read(app);
        view.selected_folder.as_ref() == Some(&inbox_reference)
            && matches!(
                &view.message_detail_state,
                MessageDetailState::Opened(opened)
                    if opened.message().id() == &inbox_message_id
                        && opened.loaded_from_cache()
            )
    })
    .await;
}

#[gpui_kit::test]
async fn contextual_folder_navigation_updates_mail_selection(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let inbox_reference = FolderReference::new(
        profile.id().clone(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic inbox ID is valid"),
    );
    let archive_reference = FolderReference::new(
        profile.id().clone(),
        FolderId::try_from("archive".to_owned()).expect("synthetic archive ID is valid"),
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
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");
    context.run_until_parked();
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);

    assert!(
        context
            .update(|_, app| { view.read(app).selected_folder.as_ref() == Some(&inbox_reference) })
    );
    context.update(gpui_kit::Window::focus_next);
    assert_eq!(
        context.update(|window, _| window.find_by_label("New mail").focused()),
        Some(true),
        "Tab order starts at the New mail button"
    );
    press_window_key(context, "tab");
    assert_eq!(
        context.update(|window, _| window.find_by_label("Inbox").focused()),
        Some(true),
        "Tab reaches the Inbox folder button after New mail"
    );
    press_window_key(context, "tab");
    assert_eq!(
        context.update(|window, _| window.find_by_label("Archive").focused()),
        Some(true),
        "Tab reaches the Archive folder button"
    );
    activate_focused_control(context);
    wait_for_ui_condition(context, |_, app| {
        view.read(app).selected_folder.as_ref() == Some(&archive_reference)
    })
    .await;
    draw_mail_view(context);

    assert!(context.update(|window, _| {
        window.try_find_by_label("Inbox").is_some() && window.try_find_by_label("Archive").is_some()
    }));
}

#[gpui_kit::test]
async fn initial_mail_snapshot_renders_loading_without_selected_folder(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let (started_receiver, release) = cache.hold_next_folder_list();
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    started_receiver
        .await
        .expect("initial folder listing reaches its test gate");

    assert!(context.cx.update(|cx| {
        view.read(cx).selected_folder.is_none()
            && matches!(&view.read(cx).message_list_state, MessageListState::Loading)
    }));
    draw_mail_view(context);

    assert!(context.debug_bounds("mail-list-loading").is_some());
    release
        .send(())
        .expect("the initial folder listing is waiting for release");
    context.run_until_parked();
}

#[gpui_kit::test]
fn initial_mail_snapshot_failure_renders_error_without_selected_folder(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .list_folders_error = Some(MailCacheError::Unavailable);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    context.run_until_parked();
    draw_mail_view(context);

    assert!(context.cx.update(|cx| {
        view.read(cx).selected_folder.is_none()
            && matches!(
                &view.read(cx).message_list_state,
                MessageListState::Failed(MailWorkflowError::Cache(MailCacheError::Unavailable))
            )
    }));
    assert!(context.debug_bounds("mail-list-error").is_none());
    assert!(context.debug_bounds("mail-list-empty-folders").is_none());
    let error_announcement_counts = context.update(|window, _| {
        let message = mail_error_message(MailWorkflowError::Cache(MailCacheError::Unavailable));
        let count_matching_message = |role| {
            window
                .find_all_by_role(role)
                .iter()
                .filter(|announcement| announcement.label() == Some(message))
                .count()
        };
        (
            count_matching_message(gpui_kit::Role::Alert),
            count_matching_message(gpui_kit::Role::Status),
        )
    });
    assert_eq!(
        error_announcement_counts,
        (1, 0),
        "the initial cache error appears once in the header alert and not in the empty list"
    );
}

#[gpui_kit::test]
fn successful_empty_mail_snapshot_renders_no_folders(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .clear();
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    context.run_until_parked();
    draw_mail_view(context);

    assert!(context.cx.update(|cx| {
        view.read(cx).selected_folder.is_none()
            && matches!(&view.read(cx).message_list_state, MessageListState::Ready)
    }));
    assert!(context.debug_bounds("mail-list-empty-folders").is_some());
    assert!(context.debug_bounds("mail-list-loading").is_none());
    assert!(context.debug_bounds("mail-list-error").is_none());
}

#[gpui_kit::test]
fn cache_snapshot_failure_survives_a_successful_sync(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    context.run_until_parked();

    context.cx.update(|cx| {
        view.update(cx, |view, _| {
            let _ = view.finish_sync(
                Ok(()),
                Err(MailWorkflowError::Cache(MailCacheError::Unavailable)),
                None,
            );
        });
    });
    let preserved_error = context.cx.update(|cx| {
        matches!(
            &view.read(cx).sync_status,
            SyncStatus::Failed(MailWorkflowError::Cache(MailCacheError::Unavailable))
        )
    });

    assert!(preserved_error);
}

#[gpui_kit::test]
fn reset_progress_is_announced_in_the_mail_status(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    context.run_until_parked();

    context.cx.update(|cx| {
        view.update(cx, |view, _| {
            view.apply_sync_progress(MailSyncProgress::Rebuilding);
        });
    });
    let status = context.cx.update(|cx| view.read(cx).sync_status_label());

    assert!(status.starts_with("Rebuilding mail"));
}

#[gpui_kit::test]
fn failed_rebuild_status_explains_how_to_resume(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    {
        let mut data = cache.0.lock().expect("fixture cache mutex is healthy");
        let current = data.sync_state.as_ref().expect("fixture sync state exists");
        data.sync_state = Some(
            MailSyncState::from_checkpoint(
                current.cutoff(),
                current.delta_link().cloned(),
                Some(MessageDeltaLink::from_opaque(
                    "https://graph.microsoft.com/v1.0/delta?token=synthetic-reset".to_owned(),
                )),
                current.last_synchronized_at(),
                Some(1),
                1,
            )
            .expect("synthetic rebuild checkpoint is valid"),
        );
    }
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    context.run_until_parked();

    let status = context.cx.update(|cx| view.read(cx).sync_status_label());

    assert!(status.contains("saved mail rebuild is incomplete"));
    assert!(status.contains("Choose Sync to resume it."));
}
