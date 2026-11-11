use super::*;

#[gpui_kit::test]
async fn reply_composer_uses_the_cached_source_and_requires_an_explicit_send(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_mail_composer_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account.clone());
    let can_send = Arc::new(AtomicBool::new(false));
    let send_calls = Arc::new(AtomicUsize::new(0));
    let reply_sources = Arc::new(Mutex::new(Vec::new()));
    let permission_requests = Arc::new(AtomicUsize::new(0));
    let provider = PermissionProvider {
        offline: OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        can_send: Arc::clone(&can_send),
        send_calls: Arc::clone(&send_calls),
        send_gate: Arc::default(),
        reply_sources: Arc::clone(&reply_sources),
        reply_error: Arc::default(),
    };
    let requester: Arc<dyn MailPermissionRequester> = Arc::new(FixturePermissionRequester {
        account_id: account.clone(),
        requests: Arc::clone(&permission_requests),
        gate: Mutex::new(None),
    });
    let workflow = Arc::new(MailWorkflow::new(profile.id().clone(), provider, cache));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| MailView::new(profile, workflow, requester, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    let row = context
        .debug_bounds("message-row-message-1")
        .expect("cached source message is rendered");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-actions")
    })
    .await;
    let reply_action = context
        .debug_bounds("message-reply")
        .expect("opened cached message exposes a native Reply action");
    context.simulate_click(reply_action.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    assert!(context.debug_bounds("mail-reply-context").is_some());
    assert!(context.debug_bounds("mail-recipient-input").is_none());
    assert!(context.debug_bounds("mail-subject-input").is_none());
    let body = context
        .debug_bounds("mail-body-input")
        .expect("reply composer exposes a plain-text body");
    context.simulate_click(body.center(), Modifiers::default());
    context.simulate_input("Synthetic private reply body");
    let send = context
        .debug_bounds("mail-send-button")
        .expect("reply composer exposes an explicit Send action");
    context.simulate_click(send.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "mail-composer-status")
    })
    .await;
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);
    let review = context
        .debug_bounds("review-send-permission")
        .expect("permission denial offers an explicit Mail.Send review");
    context.simulate_click(review.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    assert_eq!(permission_requests.load(Ordering::Relaxed), 1);
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);

    can_send.store(true, Ordering::Relaxed);
    let send = context
        .debug_bounds("mail-send-button")
        .expect("permission review leaves the reply available");
    context.simulate_click(send.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| send_calls.load(Ordering::Relaxed) == 2).await;

    assert_eq!(send_calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        reply_sources
            .lock()
            .expect("fixture reply-source mutex is healthy")
            .as_slice(),
        [
            (
                account.as_str().to_owned(),
                "inbox".to_owned(),
                message_id.as_str().to_owned(),
            ),
            (
                account.as_str().to_owned(),
                "inbox".to_owned(),
                message_id.as_str().to_owned(),
            ),
        ]
    );
}

#[gpui_kit::test]
async fn unknown_reply_requires_sent_items_review_and_duplicate_confirmation(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_mail_composer_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let send_calls = Arc::new(AtomicUsize::new(0));
    let reply_sources = Arc::new(Mutex::new(Vec::new()));
    let provider = PermissionProvider {
        offline: OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        can_send: Arc::new(AtomicBool::new(true)),
        send_calls: Arc::clone(&send_calls),
        send_gate: Arc::default(),
        reply_sources: Arc::clone(&reply_sources),
        reply_error: Arc::new(Mutex::new(Some(MailSendProviderError::OutcomeUnknown))),
    };
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
        .expect("cached source message is rendered");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-actions")
    })
    .await;
    let reply_action = context
        .debug_bounds("message-reply")
        .expect("opened cached message exposes a native Reply action");
    context.simulate_click(reply_action.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    let body = context
        .debug_bounds("mail-body-input")
        .expect("reply composer exposes its body");
    context.simulate_click(body.center(), Modifiers::default());
    context.simulate_input("Synthetic private reply body");
    let send = context
        .debug_bounds("mail-send-button")
        .expect("reply composer exposes its Send action");
    context.simulate_click(send.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "mail-pending-send")
    })
    .await;
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);
    assert!(context.debug_bounds("mail-pending-send").is_some());

    let send = context
        .debug_bounds("mail-send-button")
        .expect("unresolved reply keeps Send visible");
    context.simulate_click(send.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);
    let resolve = context
        .debug_bounds("resolve-pending-mail-send")
        .expect("unknown reply requires explicit Sent Items review");
    context.simulate_click(resolve.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "mail-composer-status")
            && !test_element_is_visible(window, "mail-pending-send")
    })
    .await;
    assert!(context.debug_bounds("mail-pending-send").is_none());
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);
    let confirm = context
        .debug_bounds("confirm-duplicate-risk")
        .expect("resolved unknown outcome requires duplicate-risk acknowledgement");
    context.simulate_click(confirm.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    let send = context
        .debug_bounds("mail-send-button")
        .expect("confirmed duplicate risk allows a separate reply");
    context.simulate_click(send.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| send_calls.load(Ordering::Relaxed) == 2).await;

    assert_eq!(send_calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        reply_sources
            .lock()
            .expect("fixture reply-source mutex is healthy")
            .iter()
            .map(|source| source.2.as_str())
            .collect::<Vec<_>>(),
        [message_id.as_str(), message_id.as_str()]
    );
}

#[gpui_kit::test]
async fn permission_review_requires_a_separate_send_action(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_mail_composer_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account.clone());
    let can_send = Arc::new(AtomicBool::new(false));
    let send_calls = Arc::new(AtomicUsize::new(0));
    let provider = PermissionProvider {
        offline: OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        can_send: Arc::clone(&can_send),
        send_calls: Arc::clone(&send_calls),
        send_gate: Arc::default(),
        reply_sources: Arc::default(),
        reply_error: Arc::default(),
    };
    let permission_requests = Arc::new(AtomicUsize::new(0));
    let (permission_started, permission_started_receiver) = tokio::sync::oneshot::channel();
    let (permission_release, permission_release_receiver) = tokio::sync::oneshot::channel();
    let requester: Arc<dyn MailPermissionRequester> = Arc::new(FixturePermissionRequester {
        account_id: account,
        requests: Arc::clone(&permission_requests),
        gate: Mutex::new(Some(PermissionGate {
            started: permission_started,
            release: permission_release_receiver,
        })),
    });
    let workflow = Arc::new(MailWorkflow::new(profile.id().clone(), provider, cache));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| MailView::new(profile, workflow, requester, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    let compose = context
        .debug_bounds("new-mail-message")
        .expect("mail view exposes the compose action");
    context.simulate_click(compose.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    for (selector, value) in [
        ("mail-recipient-input", "ada@example.test"),
        ("mail-subject-input", "Synthetic subject"),
        ("mail-body-input", "Synthetic private body"),
    ] {
        let bounds = context
            .debug_bounds(selector)
            .expect("composer renders each message field");
        context.simulate_click(bounds.center(), Modifiers::default());
        context.simulate_input(value);
    }
    let send = context
        .debug_bounds("mail-send-button")
        .expect("composer exposes its explicit send action");
    context.simulate_click(send.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "mail-composer-status")
    })
    .await;
    let review = context
        .debug_bounds("review-send-permission")
        .expect("permission denial offers an explicit review action");
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);
    context.simulate_click(review.center(), Modifiers::default());
    permission_started_receiver
        .await
        .expect("permission review reaches its held requester");
    context.run_until_parked();
    draw_mail_view(context);
    context.simulate_keystrokes("escape");
    let remains_open_after_permission_escape =
        context.update(|window, cx| window.has_active_sheet(cx));
    click_composer_sheet_close(context);
    let remains_open_after_permission_close =
        context.update(|window, cx| window.has_active_sheet(cx));
    permission_release
        .send(())
        .expect("the permission requester is waiting for release");

    wait_for_ui_condition(context, |window, app| {
        !view
            .read(app)
            .composer
            .as_ref()
            .is_some_and(|composer| composer.read(app).operation_in_progress())
            && test_element_is_visible(window, "mail-composer-status")
    })
    .await;
    assert!(remains_open_after_permission_escape);
    assert!(remains_open_after_permission_close);
    assert_eq!(permission_requests.load(Ordering::Relaxed), 1);
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);
    can_send.store(true, Ordering::Relaxed);
    context.run_until_parked();
    draw_mail_view(context);
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);

    let send = context
        .debug_bounds("mail-send-button")
        .expect("permission review leaves the draft available");
    context.simulate_click(send.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    assert_eq!(send_calls.load(Ordering::Relaxed), 2);
}

#[gpui_kit::test]
async fn pending_send_cannot_be_dismissed_by_escape_or_outside_click(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_mail_composer_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let provider = PermissionProvider {
        offline: OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        can_send: Arc::new(AtomicBool::new(true)),
        send_calls: Arc::new(AtomicUsize::new(0)),
        send_gate: Arc::default(),
        reply_sources: Arc::default(),
        reply_error: Arc::default(),
    };
    let (send_started, release_send) = provider.hold_next_send();
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider.clone(),
        cache,
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    wait_for_mail_sync_to_finish(context, &view).await;
    let compose = context
        .debug_bounds("new-mail-message")
        .expect("mail view exposes the compose action");
    context.simulate_click(compose.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    for (selector, value) in [
        ("mail-recipient-input", "ada@example.test"),
        ("mail-subject-input", "Synthetic subject"),
        ("mail-body-input", "Synthetic private body"),
    ] {
        let bounds = context
            .debug_bounds(selector)
            .expect("composer renders each message field");
        context.simulate_click(bounds.center(), Modifiers::default());
        context.simulate_input(value);
    }
    let send = context
        .debug_bounds("mail-send-button")
        .expect("composer exposes its explicit send action");
    context.simulate_click(send.center(), Modifiers::default());
    send_started
        .await
        .expect("synthetic send reaches its held provider");
    context.run_until_parked();
    draw_mail_view(context);

    let compose = context
        .debug_bounds("new-mail-message")
        .expect("mail view remains visible behind the composer");
    context.simulate_click(compose.center(), Modifiers::default());
    let remains_open_after_click = context.update(|window, cx| window.has_active_sheet(cx));
    context.simulate_keystrokes("escape");
    let remains_open_after_escape = context.update(|window, cx| window.has_active_sheet(cx));
    click_composer_sheet_close(context);
    let remains_open_after_title_close = context.update(|window, cx| window.has_active_sheet(cx));
    release_send
        .send(())
        .expect("the synthetic mail send is waiting for release");

    assert!(remains_open_after_click);
    assert!(remains_open_after_escape);
    assert!(remains_open_after_title_close);
    assert_eq!(provider.send_calls.load(Ordering::Relaxed), 1);
}

#[gpui_kit::test]
async fn send_staging_failure_skips_provider_and_accepted_cleanup_failure_can_be_resolved(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_mail_composer_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account.clone());
    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .fail_next_pending_send_stage = true;
    let send_calls = Arc::new(AtomicUsize::new(0));
    let provider = PermissionProvider {
        offline: OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        can_send: Arc::new(AtomicBool::new(true)),
        send_calls: Arc::clone(&send_calls),
        send_gate: Arc::default(),
        reply_sources: Arc::default(),
        reply_error: Arc::default(),
    };
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
    let compose = context
        .debug_bounds("new-mail-message")
        .expect("mail view exposes the compose action");
    context.simulate_click(compose.center(), Modifiers::default());
    draw_mail_view(context);
    for (selector, value) in [
        ("mail-recipient-input", "ada@example.test"),
        ("mail-subject-input", "Synthetic subject"),
        ("mail-body-input", "Synthetic private body"),
    ] {
        let bounds = context
            .debug_bounds(selector)
            .expect("composer renders each message field");
        context.simulate_click(bounds.center(), Modifiers::default());
        context.simulate_input(value);
    }
    let send = context
        .debug_bounds("mail-send-button")
        .expect("composer exposes its explicit send action");
    context.simulate_click(send.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, app| {
        !view
            .read(app)
            .composer
            .as_ref()
            .is_some_and(|composer| composer.read(app).operation_in_progress())
            && test_element_is_visible(window, "mail-composer-status")
    })
    .await;
    assert_eq!(send_calls.load(Ordering::Relaxed), 0);
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).pending_send, Ok(None)))
    );
    let send = context
        .debug_bounds("mail-send-button")
        .expect("staging failure leaves the draft available for another attempt");
    cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .fail_next_pending_send_clear = true;
    context.simulate_click(send.center(), Modifiers::default());

    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "mail-pending-send")
    })
    .await;
    let resolve = context
        .debug_bounds("resolve-pending-mail-send")
        .expect("accepted send with a failed cleanup exposes an explicit resolution");
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).pending_send, Ok(Some(_))))
    );
    context.simulate_click(resolve.center(), Modifiers::default());

    wait_for_ui_condition(context, |_, app| {
        matches!(&view.read(app).pending_send, Ok(None))
    })
    .await;
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).pending_send, Ok(None)))
    );
    assert!(context.debug_bounds("resolve-pending-mail-send").is_none());
    assert_eq!(send_calls.load(Ordering::Relaxed), 1);
}
