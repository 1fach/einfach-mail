use super::*;

#[gpui_kit::test]
fn calendar_workspace_puts_new_event_and_the_month_in_the_navigation_pane(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let session: Arc<dyn AccountSession> = Arc::new(TestAccountSession {
        account_id: synthetic_account_id(),
        removal_attempts: Arc::new(Mutex::new(Vec::new())),
    });
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
        session,
        fallback_removals: Arc::new(AtomicBool::new(false)),
        session_open_attempts: Arc::new(AtomicUsize::new(0)),
        opened_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let (view, context) =
        cx.add_window_view(|_, cx| Workspace::new(Ok(AccountServices::new(workflow, factory)), cx));
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

    let navigation = context.update(|window, _| window.find_by_label("Main navigation").bounds());
    for selector in ["calendar-new-event", "calendar-mini-days"] {
        let bounds = context
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn while Calendar is open"));
        assert!(
            bounds.left() >= navigation.left() && bounds.right() <= navigation.right(),
            "{selector} sits in the navigation pane; bounds={bounds:?}, pane={navigation:?}"
        );
    }

    click(context, "nav-accounts");

    assert!(context.debug_bounds("calendar-mini-days").is_none());
    assert!(context.debug_bounds("calendar-new-event").is_none());
}
