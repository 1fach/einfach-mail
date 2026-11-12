use super::*;

#[gpui_kit::test]
fn calendar_cache_open_failure_uses_account_scoped_error_copy(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::InvalidData,
        open_gate: Mutex::new(None),
        sign_out_events: Arc::clone(&provider.sign_out_events),
        removal_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let (view, context) =
        cx.add_window_view(|_, cx| Workspace::new(Ok(AccountServices::new(workflow, factory)), cx));
    context.run_until_parked();

    let profile = synthetic_account_profile(
        synthetic_account_id(),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.test_set_connected_account(connected_account(profile), cx);
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "nav-calendar");

    assert!(context.debug_bounds("account-cache-error").is_some());
    assert!(context.cx.update(|cx| {
        view.read(cx)
            .test_session_open_failed_with(AccountCacheOpenError::InvalidData)
    }));
    let message =
        crate::features::test_account_cache_open_error_message(AccountCacheOpenError::InvalidData);
    assert!(message.contains("account cache"));
    assert!(!message.contains("mail"));

    click(context, "review-app-permissions");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_has_pending_permission_review(cx))
    );
    click(context, "cancel-permission-review");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_pending_permission_review_is(None, cx))
    );
}

#[gpui_kit::test]
fn stale_content_open_failure_does_not_replace_a_reconnected_account(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let (open_started, open_started_receiver) = mpsc::channel();
    let (release_open, open_release) = oneshot::channel();
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::Unavailable,
        open_gate: Mutex::new(Some(OpenSessionGate {
            started: open_started,
            release: open_release,
        })),
        sign_out_events: Arc::new(Mutex::new(Vec::new())),
        removal_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| Workspace::new(Ok(AccountServices::new(workflow, factory)), cx));
    context.run_until_parked();
    let profile = synthetic_account_profile(
        synthetic_account_id(),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid");
    let replacement_account_id = AccountId::try_from("replacement-account".to_owned())
        .expect("replacement account ID is valid");
    let replacement_profile = AccountProfile::try_new(
        replacement_account_id.clone(),
        Some("Lin Example".to_owned()),
        Some("lin@example.test".to_owned()),
        Some("lin@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("replacement account profile is valid");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.test_set_connected_account(connected_account(profile), cx);
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "nav-mail");
    open_started_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("the account session open starts");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.test_set_opening_mail_content(cx);
            view.test_set_connected_account(connected_account(replacement_profile), cx);
            cx.notify();
        });
    });
    release_open
        .send(())
        .expect("the synthetic account session open is waiting on this test");
    context.run_until_parked();

    let stale_error_ignored = context.cx.update(|cx| {
        let view = view.read(cx);
        view.test_status_is_connected_to(&replacement_account_id, cx)
            && view.test_content() == WorkspaceTestContent::OpeningMail
            && view.test_session_open_error_is_none()
    });
    assert!(stale_error_ignored);
}

#[gpui_kit::test]
fn restore_to_another_account_opens_and_removes_its_session(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = Arc::clone(&provider.signed_out);
    let account_a_id = AccountId::try_from("account-a".to_owned()).expect("account ID is valid");
    let account_b_id = AccountId::try_from("account-b".to_owned()).expect("account ID is valid");
    let profile_a = AccountProfile::try_new(
        account_a_id.clone(),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid");
    let profile_b = AccountProfile::try_new(
        account_b_id.clone(),
        Some("Lin Example".to_owned()),
        Some("lin@example.test".to_owned()),
        Some("lin@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid");
    let session_a_removals = Arc::new(Mutex::new(Vec::new()));
    let session_b_removals = Arc::new(Mutex::new(Vec::new()));
    let session_a: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: account_a_id,
        removal_attempts: Arc::clone(&session_a_removals),
    });
    let session_b: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: account_b_id.clone(),
        removal_attempts: Arc::clone(&session_b_removals),
    });
    let session_open_attempts = Arc::new(AtomicUsize::new(0));
    let opened_account_ids = Arc::new(Mutex::new(Vec::new()));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
        session: Arc::clone(&session_b),
        fallback_removals: Arc::new(AtomicBool::new(false)),
        session_open_attempts,
        opened_account_ids: Arc::clone(&opened_account_ids),
    });
    let workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let (view, context) =
        cx.add_window_view(|_, cx| Workspace::new(Ok(AccountServices::new(workflow, factory)), cx));
    context.run_until_parked();
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.test_set_connected_account(connected_account(profile_a), cx);
            view.test_set_account_session(Some(Arc::clone(&session_a)));
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "sign-out");

    assert!(context.cx.update(|cx| {
        view.read(cx)
            .test_status_is_failed_with_possible_saved_credential(
                AccountOperationError::SavedAccountIdentityUnavailable,
                cx,
            )
    }));
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_session_matches(&session_a))
    );

    *provider
        .saved_account_id
        .lock()
        .expect("synthetic saved account ID mutex is healthy") = Some(account_b_id.clone());
    *provider
        .restore_profile
        .lock()
        .expect("synthetic restored profile mutex is healthy") = Some(profile_b.clone());
    click(context, "retry-account-restore");

    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_status_is_connected_to(&account_b_id, cx))
    );
    assert!(
        context
            .cx
            .update(|cx| !view.read(cx).test_has_account_session())
    );

    click(context, "nav-mail");

    assert_eq!(
        *opened_account_ids
            .lock()
            .expect("synthetic opened-account ID mutex is healthy"),
        vec![account_b_id.clone()]
    );
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_session_matches(&session_b))
    );

    click(context, "nav-accounts");
    click(context, "sign-out");

    let retry_state_is_bound_to_account_b = context.cx.update(|cx| {
        let view = view.read(cx);
        view.test_status_is_connected_to(&account_b_id, cx) && view.test_session_matches(&session_b)
    });
    assert!(retry_state_is_bound_to_account_b);
    assert!(
        session_a_removals
            .lock()
            .expect("synthetic account A removal log mutex is healthy")
            .is_empty()
    );
    assert_eq!(
        *session_b_removals
            .lock()
            .expect("synthetic account B removal log mutex is healthy"),
        vec![account_b_id.clone()]
    );

    click(context, "sign-out");

    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_status_is_disconnected(cx))
    );
    assert!(signed_out.load(Ordering::SeqCst));
    assert_eq!(
        *session_b_removals
            .lock()
            .expect("synthetic account B removal log mutex is healthy"),
        vec![account_b_id.clone(), account_b_id]
    );
}

#[gpui_kit::test]
fn sign_out_after_calendar_return_reuses_the_retained_session(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = Arc::clone(&provider.signed_out);
    provider.fail_next_sign_out.store(true, Ordering::SeqCst);
    *provider
        .saved_account_id
        .lock()
        .expect("synthetic saved account ID mutex is healthy") = Some(synthetic_account_id());
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let removal_attempts = Arc::new(Mutex::new(Vec::new()));
    let fallback_removals = Arc::new(AtomicBool::new(false));
    let session: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: synthetic_account_id(),
        removal_attempts: Arc::clone(&removal_attempts),
    });
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
        session: Arc::clone(&session),
        fallback_removals: Arc::clone(&fallback_removals),
        session_open_attempts: Arc::new(AtomicUsize::new(0)),
        opened_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| Workspace::new(Ok(AccountServices::new(workflow, factory)), cx));
    context.run_until_parked();
    let profile = synthetic_account_profile(
        synthetic_account_id(),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid");
    let account_id = profile.id().clone();
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.test_set_connected_account(connected_account(profile), cx);
            cx.notify();
        });
    });
    draw_account_view(context);

    click(context, "nav-calendar");
    let calendar_open = context
        .cx
        .update(|cx| view.read(cx).test_content() == WorkspaceTestContent::Calendar);
    assert!(calendar_open);

    click(context, "nav-accounts");
    let returned_to_account = context
        .cx
        .update(|cx| view.read(cx).test_content() == WorkspaceTestContent::Accounts);
    assert!(returned_to_account);

    let session_retained_after_return = context
        .cx
        .update(|cx| view.read(cx).test_session_matches(&session));
    assert!(session_retained_after_return);

    click(context, "sign-out");

    let retry_state_kept = context.cx.update(|cx| {
        let account_view = view.read(cx);
        account_view.test_status_is_connected_to(&account_id, cx)
            && account_view.test_session_matches(&session)
    });
    assert!(retry_state_kept);
    assert_eq!(
        *removal_attempts
            .lock()
            .expect("synthetic session removal log mutex is healthy"),
        vec![account_id.clone()]
    );
    assert!(!fallback_removals.load(Ordering::SeqCst));
    assert!(!signed_out.load(Ordering::SeqCst));

    click(context, "sign-out");

    let session_retained_after_credential_failure = context
        .cx
        .update(|cx| view.read(cx).test_session_matches(&session));
    assert!(session_retained_after_credential_failure);
    assert!(!signed_out.load(Ordering::SeqCst));

    click(context, "sign-out");

    let state_cleared = context.cx.update(|cx| {
        let account_view = view.read(cx);
        account_view.test_status_is_disconnected(cx) && !account_view.test_has_account_session()
    });
    assert!(state_cleared);
    assert!(signed_out.load(Ordering::SeqCst));
    assert_eq!(
        *removal_attempts
            .lock()
            .expect("synthetic session removal log mutex is healthy"),
        vec![account_id.clone(), account_id.clone(), account_id]
    );
    assert!(!fallback_removals.load(Ordering::SeqCst));
}
