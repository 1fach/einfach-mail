use super::*;

#[gpui_kit::test]
fn connects_and_signs_out_through_the_account_view(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = provider.signed_out.clone();
    let sign_out_events = Arc::clone(&provider.sign_out_events);
    let event_log = Arc::clone(&sign_out_events);
    let removal_account_ids = Arc::new(Mutex::new(Vec::new()));
    provider.fail_next_sign_out.store(true, Ordering::SeqCst);
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::Unavailable,
        open_gate: Mutex::new(None),
        sign_out_events: Arc::clone(&sign_out_events),
        removal_account_ids: Arc::clone(&removal_account_ids),
    });
    let (view, context) = cx
        .add_window_view(|_, cx| AccountView::new(Ok(AccountServices::new(workflow, factory)), cx));

    context.run_until_parked();
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Restoring;
            cx.notify();
        });
    });
    draw_account_view(context);
    assert!(context.debug_bounds("account-operation-progress").is_some());
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).account_header_description()),
        "Checking for a saved account."
    );
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Disconnected;
            cx.notify();
        });
    });
    draw_account_view(context);
    assert!(context.debug_bounds("connect-account").is_some());
    assert!(
        context
            .debug_bounds("microsoft365-provider-description")
            .is_some()
    );
    assert!(
        context
            .debug_bounds("exchange-ews-provider-description")
            .is_some()
    );
    assert!(context.debug_bounds("account-requested-access").is_some());
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).account_header_description()),
        "Choose Microsoft 365 or Exchange Server to connect an account."
    );
    click(context, "connect-account");
    let connected = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::Connected(account)
                if account.provider_kind() == AccountProviderKind::MicrosoftGraph
                    && account.profile().label() == "Ada Example"
        )
    });
    assert!(connected);
    assert!(
        context
            .debug_bounds("microsoft365-provider-description")
            .is_none()
    );
    assert!(
        context
            .debug_bounds("exchange-ews-provider-description")
            .is_none()
    );
    assert!(context.debug_bounds("account-requested-access").is_none());
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).account_header_description()),
        "Manage the active account, review permissions, or sign out."
    );
    assert!(context.debug_bounds("account-provider").is_some());
    assert!(context.debug_bounds("account-mail-capabilities").is_some());
    assert!(
        context
            .debug_bounds("account-calendar-capabilities")
            .is_some()
    );

    click(context, "sign-out");
    let retryable_failure = context.cx.update(|cx| {
        matches!(
            &view.read(cx).status,
            super::AccountStatus::SignOutFailed {
                account: Some(account),
                error: AccountOperationError::CredentialStoreUnavailable,
            } if account.provider_kind() == AccountProviderKind::MicrosoftGraph
        )
    });
    assert!(retryable_failure);
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).account_header_description()),
        "Sign-out needs attention. Retry it below."
    );
    assert!(
        context
            .debug_bounds("microsoft365-provider-description")
            .is_none()
    );
    assert!(
        context
            .debug_bounds("exchange-ews-provider-description")
            .is_none()
    );
    assert!(context.debug_bounds("account-requested-access").is_none());
    assert!(context.debug_bounds("sign-out").is_some());
    assert!(context.debug_bounds("account-provider").is_some());
    assert!(context.debug_bounds("account-mail-capabilities").is_some());
    assert!(
        context
            .debug_bounds("account-calendar-capabilities")
            .is_some()
    );
    assert!(!signed_out.load(Ordering::SeqCst));

    click(context, "sign-out");
    let disconnected = context
        .cx
        .update(|cx| matches!(&view.read(cx).status, super::AccountStatus::Disconnected));
    assert!(disconnected);
    assert!(
        context
            .debug_bounds("microsoft365-provider-description")
            .is_some()
    );
    assert!(
        context
            .debug_bounds("exchange-ews-provider-description")
            .is_some()
    );
    assert!(context.debug_bounds("account-requested-access").is_some());
    assert!(signed_out.load(Ordering::SeqCst));
    assert_eq!(
        *removal_account_ids
            .lock()
            .expect("synthetic cache removal ID mutex is healthy"),
        vec![
            AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is valid");
            2
        ]
    );
    assert_eq!(
        *event_log
            .lock()
            .expect("synthetic sign-out event log mutex is healthy"),
        vec!["cache_removed", "cache_removed", "credential_removed"]
    );
}

#[gpui_kit::test]
fn signing_out_keeps_active_account_details_visible(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let (view, context) = cx.add_window_view(|_, cx| {
        AccountView::new(
            Ok(AccountServices::new(
                workflow,
                unused_account_session_factory(),
            )),
            cx,
        )
    });

    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::SigningOut {
                account: Some(connected_account(mail_only_profile())),
            };
            cx.notify();
        });
    });
    draw_account_view(context);

    assert!(context.debug_bounds("account-provider").is_some());
    assert!(context.debug_bounds("account-mail-capabilities").is_some());
    assert!(
        context
            .debug_bounds("account-calendar-capabilities")
            .is_some()
    );
    assert!(context.debug_bounds("account-operation-progress").is_some());
    assert!(
        context
            .debug_bounds("microsoft365-provider-description")
            .is_none()
    );
    assert!(
        context
            .debug_bounds("exchange-ews-provider-description")
            .is_none()
    );
    assert!(context.debug_bounds("account-requested-access").is_none());
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).account_header_description()),
        "Sign-out is in progress. Account details remain visible until it finishes."
    );
}

#[gpui_kit::test]
fn failed_restore_removes_cache_before_the_verified_saved_credential(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = Arc::clone(&provider.signed_out);
    *provider
        .restore_error
        .lock()
        .expect("synthetic restore error mutex is healthy") =
        Some(AccountOperationError::SignInExpired);
    let account_id = AccountId::try_from("verified-saved-account".to_owned()).expect("ID is valid");
    *provider
        .saved_account_id
        .lock()
        .expect("synthetic saved account ID mutex is healthy") = Some(account_id.clone());
    let event_log = Arc::clone(&provider.sign_out_events);
    let removal_account_ids = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::Unavailable,
        open_gate: Mutex::new(None),
        sign_out_events: Arc::clone(&event_log),
        removal_account_ids: Arc::clone(&removal_account_ids),
    });
    let (view, context) = cx
        .add_window_view(|_, cx| AccountView::new(Ok(AccountServices::new(workflow, factory)), cx));

    context.run_until_parked();
    draw_account_view(context);
    assert!(context.debug_bounds("connect-account").is_none());
    click(context, "sign-out");

    assert_eq!(
        *removal_account_ids
            .lock()
            .expect("synthetic cache removal ID mutex is healthy"),
        vec![account_id]
    );
    assert_eq!(
        *event_log
            .lock()
            .expect("synthetic sign-out event log mutex is healthy"),
        vec!["cache_removed", "credential_removed"]
    );
    assert!(signed_out.load(Ordering::SeqCst));
    assert!(
        context
            .cx
            .update(|cx| matches!(&view.read(cx).status, AccountStatus::Disconnected))
    );
}

#[gpui_kit::test]
fn failed_restore_without_verified_identity_preserves_credential(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let signed_out = Arc::clone(&provider.signed_out);
    *provider
        .restore_error
        .lock()
        .expect("synthetic restore error mutex is healthy") =
        Some(AccountOperationError::SignInExpired);
    let event_log = Arc::clone(&provider.sign_out_events);
    let removal_account_ids = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(FailingOpenAccountSessionFactory {
        open_error: AccountCacheOpenError::Unavailable,
        open_gate: Mutex::new(None),
        sign_out_events: Arc::clone(&event_log),
        removal_account_ids: Arc::clone(&removal_account_ids),
    });
    let (view, context) = cx
        .add_window_view(|_, cx| AccountView::new(Ok(AccountServices::new(workflow, factory)), cx));

    context.run_until_parked();
    click(context, "sign-out");

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).status,
        AccountStatus::Failed {
            error: AccountOperationError::SavedAccountIdentityUnavailable,
            saved_credential_state: SavedCredentialState::MayExist,
        }
    )));
    assert!(context.debug_bounds("connect-account").is_none());
    assert!(context.debug_bounds("sign-out").is_none());
    assert!(context.debug_bounds("retry-account-restore").is_some());
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).account_header_description()),
        "Retry account restore before removing or replacing the saved sign-in."
    );
    assert!(
        removal_account_ids
            .lock()
            .expect("synthetic cache removal ID mutex is healthy")
            .is_empty()
    );
    assert!(
        event_log
            .lock()
            .expect("synthetic sign-out event log mutex is healthy")
            .is_empty()
    );
    assert!(!signed_out.load(Ordering::SeqCst));
}

#[gpui_kit::test]
fn connect_mismatch_keeps_saved_sign_in_and_requires_restore(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    *provider
        .saved_account_id
        .lock()
        .expect("synthetic saved account ID mutex is healthy") =
        Some(AccountId::try_from("existing-account".to_owned()).expect("account ID is valid"));
    *provider
        .connect_error
        .lock()
        .expect("synthetic connection error mutex is healthy") =
        Some(AccountOperationError::AccountMismatch);
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let (view, context) = cx.add_window_view(|_, cx| {
        AccountView::new(
            Ok(AccountServices::new(
                workflow,
                unused_account_session_factory(),
            )),
            cx,
        )
    });

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-account");

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).status,
        AccountStatus::Failed {
            error: AccountOperationError::AccountMismatch,
            saved_credential_state: SavedCredentialState::MayExist,
        }
    )));
    assert!(context.debug_bounds("connect-account").is_none());
    assert!(context.debug_bounds("retry-account-restore").is_some());
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).account_header_description()),
        "Retry account restore before removing or replacing the saved sign-in."
    );
}
