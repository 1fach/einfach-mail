use super::*;

#[gpui_kit::test]
fn missing_client_configuration_disables_account_actions(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, context) = cx.add_window_view(|_, cx| {
        AccountView::<TestProvider>::new(Err(SetupIssue::MissingClientId), cx)
    });

    draw_account_view(context);

    assert!(context.debug_bounds("connect-account").is_none());
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).services.as_ref().err().copied()),
        Some(SetupIssue::MissingClientId)
    );
}

#[gpui_kit::test]
fn exchange_server_form_connects_without_microsoft_client_configuration(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let ews_connect_requests = Arc::clone(&provider.ews_connect_requests);
    let ews_input_valid = Arc::clone(&provider.ews_input_valid);
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let services = AccountServices::new(workflow, unused_account_session_factory())
        .with_microsoft365_setup_issue(SetupIssue::MissingClientId);
    let (workspace, context) = cx.add_window_view(|_, cx| Workspace::new(Ok(services), cx));
    let view = context
        .cx
        .update(|cx| workspace.read(cx).test_account_view());

    context.run_until_parked();
    draw_account_view(context);
    assert!(context.debug_bounds("connect-account").is_none());
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
    click(context, "connect-exchange-ews");
    draw_account_view(context);
    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).account_header_description()),
        "Complete or cancel the Exchange Server connection below."
    );

    for (selector, value) in [
        (
            "ews-endpoint-input",
            "https://exchange.example.test/EWS/Exchange.asmx",
        ),
        ("ews-mailbox-address-input", "ada@example.test"),
        ("ews-username-input", "EXAMPLE\\ada"),
        ("ews-password-input", "synthetic-only"),
    ] {
        scroll_account_action_into_view(context, selector);
        let bounds = context
            .debug_bounds(selector)
            .expect("Exchange form renders each required field");
        context.simulate_click(bounds.center(), Modifiers::default());
        context.simulate_input(value);
    }
    click(context, "submit-ews-connection");

    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).status,
        AccountStatus::Connected(account)
            if account.provider_kind() == AccountProviderKind::ExchangeEws
                && account.profile().capabilities()
                == AccountCapabilities::new(
                    MailCapabilities::from_actions([MailAction::SetReadState]),
                    CalendarCapabilities::view_only(),
                )
    )));
    assert!(context.debug_bounds("account-provider").is_some());
    assert!(context.debug_bounds("account-mail-capabilities").is_some());
    assert!(
        context
            .debug_bounds("account-calendar-capabilities")
            .is_some()
    );
    assert_eq!(ews_connect_requests.load(Ordering::SeqCst), 1);
    assert!(ews_input_valid.load(Ordering::SeqCst));
    assert!(context.debug_bounds("nav-mail").is_some());
    assert!(context.debug_bounds("nav-calendar").is_some());

    click(context, "nav-calendar");
    context.run_until_parked();
    draw_account_view(context);
    assert!(context.debug_bounds("nav-accounts").is_some());
    assert!(context.debug_bounds("calendar-sync").is_some());
}

#[gpui_kit::test]
fn exchange_autodiscover_shows_external_destination_before_continuing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let provider = TestProvider::default();
    let discovery_requests = Arc::clone(&provider.ews_discovery_requests);
    let discovery_input_valid = Arc::clone(&provider.ews_discovery_input_valid);
    let workflow = Arc::new(AccountWorkflow::new(provider));
    let services = AccountServices::new(workflow, unused_account_session_factory());
    let (workspace, context) = cx.add_window_view(|_, cx| Workspace::new(Ok(services), cx));
    let view = context
        .cx
        .update(|cx| workspace.read(cx).test_account_view());

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-exchange-ews");
    scroll_account_action_into_view(context, "ews-mailbox-address-input");
    let bounds = context
        .debug_bounds("ews-mailbox-address-input")
        .expect("mailbox address field is visible");
    context.simulate_click(bounds.center(), Modifiers::default());
    context.simulate_input("ada@example.test");
    click(context, "discover-ews-endpoint");

    assert_eq!(discovery_requests.load(Ordering::SeqCst), 1);
    assert!(discovery_input_valid.load(Ordering::SeqCst));
    assert!(context.debug_bounds("continue-ews-discovery").is_some());
    assert!(context.debug_bounds("cancel-ews-discovery").is_some());

    click(context, "continue-ews-discovery");

    let discovered_endpoint = context.cx.update(|cx| {
        view.read(cx)
            .exchange_ews_form
            .as_ref()
            .map(|form| form.endpoint.read(cx).value().to_string())
    });
    assert_eq!(
        discovered_endpoint.as_deref(),
        Some("https://exchange.example.test/EWS/Exchange.asmx")
    );
    assert!(context.debug_bounds("continue-ews-discovery").is_none());
}

#[gpui_kit::test]
fn exchange_form_cancel_drops_the_active_discovery_task(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let services = AccountServices::new(workflow, unused_account_session_factory());
    let (workspace, context) = cx.add_window_view(|_, cx| Workspace::new(Ok(services), cx));
    let view = context
        .cx
        .update(|cx| workspace.read(cx).test_account_view());

    context.run_until_parked();
    draw_account_view(context);
    click(context, "connect-exchange-ews");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.exchange_ews_form
                .as_mut()
                .expect("Exchange form is open")
                .discovery = super::super::EwsDiscoveryState::Searching;
            view.task = Some(gpui_kit::Task::ready(()));
            cx.notify();
        });
    });
    draw_account_view(context);

    context.cx.update(|cx| {
        view.update(cx, |view, cx| view.cancel_exchange_ews_form(cx));
    });

    assert!(context.cx.update(|cx| {
        let view = view.read(cx);
        view.exchange_ews_form.is_none() && view.task.is_none()
    }));
    assert!(context.debug_bounds("connect-exchange-ews").is_some());
}
