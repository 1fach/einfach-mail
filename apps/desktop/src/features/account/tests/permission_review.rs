use super::*;

#[gpui_kit::test]
fn partial_mail_profile_rejects_graph_permission_review(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let (view, _) = cx.add_window_view(|_, cx| {
        AccountView::new(
            Ok(AccountServices::new(
                workflow,
                unused_account_session_factory(),
            )),
            cx,
        )
    });
    let profile = AccountProfile::try_new(
        AccountId::try_from("synthetic-ews-account".to_owned())
            .expect("synthetic EWS account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@example.test".to_owned()),
        AccountCapabilities::new(
            MailCapabilities::from_actions([MailAction::SetReadState]),
            CalendarCapabilities::view_only(),
        ),
    )
    .expect("synthetic account profile has an identity");

    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(connected_account(profile));
            view.prepare_permission_review(AccountPermissionRequest::MailReadWrite, cx);
        });
    });

    assert!(cx.update(|cx| view.read(cx).pending_permission_review.is_none()));
}

#[gpui_kit::test]
fn mail_and_calendar_permission_reviews_work_after_content_opens(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let session_open_attempts = Arc::new(AtomicUsize::new(0));
    let session: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid"),
        removal_attempts: Arc::new(Mutex::new(Vec::new())),
    });
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
        session: Arc::clone(&session),
        fallback_removals: Arc::new(AtomicBool::new(false)),
        session_open_attempts: Arc::clone(&session_open_attempts),
        opened_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| Workspace::new(Ok(AccountServices::new(workflow, factory)), cx));
    context.run_until_parked();
    let profile = AccountProfile::try_new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
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

    click(context, "nav-mail");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_mail_view().is_some())
    );

    let mail_view = context
        .cx
        .update(|cx| view.read(cx).test_mail_view().expect("mail view is open"));
    context.cx.update(|cx| {
        mail_view.update(cx, |_, cx| {
            cx.emit(crate::features::MailViewEvent::ReviewMailWritePermission);
        });
    });
    context.run_until_parked();
    draw_account_view(context);
    assert!(context.cx.update(|cx| {
        let workspace = view.read(cx);
        workspace.test_mail_view().is_none()
            && workspace.test_pending_permission_review_is(
                Some(AccountPermissionRequest::MailReadWrite),
                cx,
            )
    }));
    click(context, "cancel-permission-review");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_pending_permission_review_is(None, cx))
    );

    click(context, "nav-mail");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_mail_view().is_some())
    );
    click(context, "nav-calendar");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_calendar_view().is_some())
    );
    click(context, "nav-mail");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_mail_view().is_some())
    );
    assert_eq!(session_open_attempts.load(Ordering::SeqCst), 1);
    click(context, "nav-accounts");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_mail_view().is_none())
    );
    assert!(
        context
            .cx
            .update(|cx| { view.read(cx).test_session_matches(&session) })
    );

    click(context, "nav-calendar");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_calendar_view().is_some())
    );
    let calendar_view = context.cx.update(|cx| {
        view.read(cx)
            .test_calendar_view()
            .expect("calendar view is open")
    });
    context.cx.update(|cx| {
        calendar_view.update(cx, |_, cx| {
            cx.emit(crate::features::CalendarViewEvent::ReviewCalendarWritePermission);
        });
    });
    context.run_until_parked();
    draw_account_view(context);
    assert!(context.cx.update(|cx| {
        let workspace = view.read(cx);
        workspace.test_calendar_view().is_none()
            && workspace.test_pending_permission_review_is(
                Some(AccountPermissionRequest::CalendarReadWrite),
                cx,
            )
    }));
    click(context, "cancel-permission-review");
    assert!(
        context
            .cx
            .update(|cx| view.read(cx).test_pending_permission_review_is(None, cx))
    );
    assert_eq!(session_open_attempts.load(Ordering::SeqCst), 1);
}

#[gpui_kit::test]
fn connection_cancel_resolves_before_and_during_credential_commit(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    provider.pause_connection.store(true, Ordering::SeqCst);
    let workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let mut account_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| {
            Workspace::new(
                Ok(AccountServices::new(
                    workflow,
                    unused_account_session_factory(),
                )),
                cx,
            )
        });
        account_view = Some(workspace.read(cx).test_account_view());
        gpui_kit::base::Root::new(workspace, window, cx)
    });
    let view = account_view.expect("account view is created in its window");

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-account");
    click(context, "cancel-sign-in");

    let cancelled = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::Failed {
                error: AccountOperationError::SignInCancelled,
                saved_credential_state: super::SavedCredentialState::Absent,
            }
        )
    });
    assert!(cancelled);

    provider.pause_connection.store(false, Ordering::SeqCst);
    let (commit_started, finish_commit) = provider.pause_next_connection_commit();
    click(context, "connect-account");
    commit_started
        .recv_timeout(Duration::from_secs(1))
        .expect("retry reaches the credential commit");
    click(context, "cancel-sign-in");

    let cancelling = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::CancellingConnect
        )
    });
    assert!(cancelling);
    assert!(context.debug_bounds("account-operation-progress").is_some());

    finish_commit
        .send(())
        .expect("synthetic credential commit is released");
    context.run_until_parked();
    draw_account_view(context);

    let connected = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::Connected(account)
                if account.profile().label() == "Ada Example"
        )
    });
    assert!(connected);
}

#[gpui_kit::test]
fn permission_cancel_resolves_before_and_during_credential_commit(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    provider
        .pause_permission_update
        .store(true, Ordering::SeqCst);
    let workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let mut account_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| {
            Workspace::new(
                Ok(AccountServices::new(
                    workflow,
                    unused_account_session_factory(),
                )),
                cx,
            )
        });
        account_view = Some(workspace.read(cx).test_account_view());
        gpui_kit::base::Root::new(workspace, window, cx)
    });
    let view = account_view.expect("account view is created in its window");

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-account");
    click(context, "review-app-permissions");
    click(context, "continue-permission-review");
    click(context, "cancel-permission-update");

    let cancelled_with_profile = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::ReauthorizationFailed {
                account,
                error: AccountOperationError::SignInCancelled,
            } if account.profile().label() == "Ada Example"
        )
    });
    assert!(cancelled_with_profile);

    provider
        .pause_permission_update
        .store(false, Ordering::SeqCst);
    let (commit_started, finish_commit) = provider.pause_next_permission_commit();
    click(context, "continue-permission-review");
    commit_started
        .recv_timeout(Duration::from_secs(1))
        .expect("permission retry reaches the credential commit");
    click(context, "cancel-permission-update");

    let cancelling_with_profile = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::CancellingReauthorization { account }
                if account.profile().label() == "Ada Example"
        )
    });
    assert!(cancelling_with_profile);
    assert!(context.debug_bounds("account-operation-progress").is_some());

    finish_commit
        .send(())
        .expect("synthetic credential commit is released");
    context.run_until_parked();
    draw_account_view(context);

    let connected = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::Connected(account)
                if account.profile().label() == "Ada Example"
        )
    });
    assert!(connected);
}

#[test]
fn reauthorization_mismatch_copy_confirms_saved_sign_in_was_preserved() {
    assert_eq!(
        AccountStatus::reauthorization_error_message(AccountOperationError::AccountMismatch),
        "Microsoft returned a different account. Your saved sign-in was not changed."
    );
}

#[gpui_kit::test]
fn mail_write_scope_requires_review_before_the_browser_flow(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let permission_request = Arc::clone(&provider.last_permission_request);
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let (workspace, context) = cx.add_window_view(|_, cx| {
        Workspace::new(
            Ok(AccountServices::new(
                workflow,
                unused_account_session_factory(),
            )),
            cx,
        )
    });
    let view = context
        .cx
        .update(|cx| workspace.read(cx).test_account_view());

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-account");
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).status, AccountStatus::Connected(_)))
    );
    context.simulate_resize(size(px(800.), px(600.)));
    draw_account_view(context);
    let viewport = context.update(|window, _| window.bounds());
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.prepare_permission_review(AccountPermissionRequest::MailReadWrite, cx);
        });
    });
    draw_account_view(context);

    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).pending_permission_review),
        Some(AccountPermissionRequest::MailReadWrite)
    );
    let can_continue = context.cx.update(|cx| {
        let view = view.read(cx);
        view.task.is_none() && !view.workspace_is_opening
    });
    assert!(can_continue);
    let disclosure =
        super::super::permission_review_disclosure(AccountPermissionRequest::MailReadWrite);
    assert!(disclosure.contains("create, read, update, and delete mail"));
    assert!(disclosure.contains("move messages between folders"));
    assert!(disclosure.contains("delete selected messages after confirmation"));
    assert!(disclosure.contains("does not request Mail.Send"));
    assert_eq!(
        *permission_request
            .lock()
            .expect("synthetic permission request mutex is not poisoned"),
        None
    );
    assert!(context.debug_bounds("continue-permission-review").is_some());
    assert!(
        context
            .debug_bounds("continue-permission-review")
            .expect("continue action is rendered")
            .bottom()
            > viewport.bottom()
    );

    scroll_account_action_into_view(context, "continue-permission-review");
    let continue_bounds = context
        .debug_bounds("continue-permission-review")
        .expect("continue action remains rendered after scrolling");
    assert!(continue_bounds.top() >= viewport.top());
    assert!(continue_bounds.bottom() <= viewport.bottom());

    click(context, "continue-permission-review");

    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).status, AccountStatus::Connected(_)))
    );

    assert_eq!(
        *permission_request
            .lock()
            .expect("synthetic permission request mutex is not poisoned"),
        Some(AccountPermissionRequest::MailReadWrite)
    );
}
