use super::*;

#[gpui_kit::test]
async fn confirmed_read_state_updates_the_view_and_encrypted_cache(cx: &mut TestAppContext) {
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
        ReadStateProvider::new(ReadStateActionResponse::Confirm, Arc::clone(&updates)),
        cache.clone(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    open_cached_message_and_click_read_state(context);

    assert_eq!(
        context.cx.update(|cx| {
            view.read(cx)
                .messages
                .iter()
                .find(|message| message.id() == &message_id)
                .map(|message| message.metadata().read_state())
        }),
        Some(ReadState::Read)
    );
    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).read_state_update,
        ReadStateUpdateStatus::Confirmed {
            status: ReadState::Read,
            ..
        }
    )));
    assert_eq!(
        updates
            .lock()
            .expect("fixture read-state log mutex is healthy")
            .as_slice(),
        [(message_id.clone(), ReadState::Read)]
    );
    assert_eq!(
        cache
            .message_read_state(&folder, &message_id)
            .await
            .expect("confirmed state is readable from the cache"),
        Some(ReadState::Read)
    );
}

#[gpui_kit::test]
fn failed_initial_mail_sync_keeps_read_state_changes_disabled(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let updates = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(ReadStateActionResponse::Confirm, Arc::clone(&updates))
            .with_sync_failure(),
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    context.run_until_parked();
    draw_mail_view(context);
    let row_bounds = context
        .debug_bounds("message-row-message-1")
        .expect("cached message remains visible after sync failure");
    context.simulate_click(row_bounds.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    let action_bounds = context
        .debug_bounds("message-read-state")
        .expect("read-state action remains visible after sync failure");
    context.simulate_click(action_bounds.center(), Modifiers::default());
    context.run_until_parked();

    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        view.selected_read_state_request().is_none()
            && matches!(&view.read_state_update, ReadStateUpdateStatus::Idle)
    }));
    assert!(context.debug_bounds("read-state-reconciliation").is_some());
    assert!(
        updates
            .lock()
            .expect("fixture read-state log mutex is healthy")
            .is_empty()
    );
    let state = context.cx.update(|cx| {
        view.read(cx)
            .messages
            .iter()
            .find(|message| message.id() == &message_id)
            .map(|message| message.metadata().read_state())
    });
    assert_eq!(state, Some(ReadState::Unread));
}

#[gpui_kit::test]
async fn confirmed_read_state_stays_visible_when_cache_write_fails(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let folder = FolderReference::new(
        account.clone(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    let (profile, cache, message_id) = fixture(account);
    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .read_state_write_error = Some(MailCacheError::Unavailable);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Confirm,
            Arc::new(Mutex::new(Vec::new())),
        ),
        cache.clone(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    open_cached_message_and_click_read_state(context);

    let (visible_state, needs_sync) = context.cx.update(|cx| {
        let view = view.read(cx);
        let visible_state = view
            .messages
            .iter()
            .find(|message| message.id() == &message_id)
            .map(|message| message.metadata().read_state());
        let needs_sync = matches!(
            &view.read_state_update,
            ReadStateUpdateStatus::ConfirmedNeedsSync {
                status: ReadState::Read,
                ..
            }
        );
        (visible_state, needs_sync)
    });
    assert_eq!(visible_state, Some(ReadState::Read));
    assert!(needs_sync);
    assert_eq!(
        cache
            .message_read_state(&folder, &message_id)
            .await
            .expect("cache remains readable after a failed write"),
        Some(ReadState::Unread)
    );
    assert!(
        context
            .debug_bounds("message-read-state-feedback")
            .is_some()
    );
}

#[gpui_kit::test]
fn uncertain_read_state_shows_sync_guidance_and_blocks_another_write(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let updates = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(ReadStateActionResponse::Unconfirmed, Arc::clone(&updates)),
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    open_cached_message_and_click_read_state(context);

    let (state, request_blocked) = context.cx.update(|cx| {
        let view = view.read(cx);
        let state = view
            .messages
            .iter()
            .find(|message| message.id() == &message_id)
            .map(|message| message.metadata().read_state());
        let request_blocked = view.selected_read_state_request().is_none();
        (state, request_blocked)
    });
    assert_eq!(state, Some(ReadState::Unread));
    assert!(request_blocked);
    assert!(
        context
            .debug_bounds("message-read-state-feedback")
            .is_some()
    );
    assert!(context.debug_bounds("read-state-reconciliation").is_some());

    let action_bounds = context
        .debug_bounds("message-read-state")
        .expect("read-state action remains visible");
    context.simulate_click(action_bounds.center(), Modifiers::default());
    context.run_until_parked();
    assert_eq!(
        updates
            .lock()
            .expect("fixture read-state log mutex is healthy")
            .as_slice(),
        [(message_id, ReadState::Read)]
    );
}

#[gpui_kit::test]
fn pending_read_state_does_not_render_duplicate_account_navigation(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Pending,
            Arc::new(Mutex::new(Vec::new())),
        ),
        cache,
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
    let action_bounds = context
        .debug_bounds("message-read-state")
        .expect("opened message exposes its read-state action");
    context.simulate_click(action_bounds.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).read_state_update,
        ReadStateUpdateStatus::Pending {
            requested: ReadState::Read,
            ..
        }
    )));

    assert!(context.debug_bounds("mail-account-navigation").is_none());
}

#[gpui_kit::test]
fn denied_read_state_offers_account_permission_review(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Deny,
            Arc::new(Mutex::new(Vec::new())),
        ),
        cache.clone(),
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    let permission_review_requested = Arc::new(AtomicBool::new(false));
    let observed_review = Arc::clone(&permission_review_requested);
    let _subscription = context.cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &MailViewEvent, _| {
            if matches!(event, MailViewEvent::ReviewMailWritePermission) {
                observed_review.store(true, Ordering::Relaxed);
            }
        })
    });

    open_cached_message_and_click_read_state(context);

    let review_bounds = context
        .debug_bounds("review-mail-permissions-from-message")
        .expect("denied write offers permission review");
    context.simulate_click(review_bounds.center(), Modifiers::default());
    context.run_until_parked();
    assert!(permission_review_requested.load(Ordering::Relaxed));
    assert_eq!(
        cache
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .messages
            .iter()
            .find(|message| message.id() == &message_id)
            .map(|message| message.metadata().read_state()),
        Some(ReadState::Unread)
    );
}

#[gpui_kit::test]
fn offline_read_state_shows_retry_guidance(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Offline,
            Arc::new(Mutex::new(Vec::new())),
        ),
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    open_cached_message_and_click_read_state(context);

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).read_state_update,
        ReadStateUpdateStatus::Failed {
            error: MailWorkflowError::Provider(MailOperationError::ServiceUnavailable),
            ..
        }
    )));
    assert!(
        context
            .debug_bounds("message-read-state-feedback")
            .is_some()
    );
}

#[test]
fn offline_read_state_failure_explains_how_to_retry() {
    assert_eq!(
        read_state_failure_message(
            ReadState::Read,
            MailWorkflowError::Provider(MailOperationError::ServiceUnavailable),
        ),
        "Couldn’t mark the message read. Check your connection and try again."
    );
}
