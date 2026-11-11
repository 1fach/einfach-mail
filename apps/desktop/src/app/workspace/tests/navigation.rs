use super::*;

#[gpui_kit::test]
fn workspace_navigation_stays_visible_and_supports_keyboard_switching(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx
        .add_window_view(|_, cx| AccountView::new(Ok(AccountServices::new(workflow, factory)), cx));
    assert_disconnected_workspace_routes(&view, context);
    connect_synthetic_workspace_account(&view, context);
    assert_keyboard_navigation_selects_calendar(&view, context);
}

#[gpui_kit::test]
fn mail_workspace_places_folders_before_the_message_list_and_reader(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx
        .add_window_view(|_, cx| AccountView::new(Ok(AccountServices::new(workflow, factory)), cx));
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(connected_account(mail_only_profile()));
            cx.notify();
        });
    });

    click(context, "nav-mail");

    assert!(
        context
            .cx
            .update(|cx| { matches!(&view.read(cx).content, AccountContent::Mail { .. }) })
    );
    assert_workspace_navigation_is_visible(context);
    let (navigation, folders, reader) = context.update(|window, _| {
        (
            window.find_by_label("Main navigation").bounds(),
            window.find_by_label("Mail folders").bounds(),
            window.find_by_label("Message reader").bounds(),
        )
    });
    let message_list = context
        .debug_bounds("mail-list-loading")
        .expect("the loading message list is rendered");
    let identity = context.update(|window, _| window.find_by_label("Active account: Ada Example"));

    assert_eq!(identity.role(), Some(gpui_kit::Role::Group));
    assert!(navigation.size.width >= px(224.));
    assert!(navigation.size.width <= px(320.));
    assert!(folders.left() >= navigation.left());
    assert!(folders.right() <= navigation.right());
    assert!(message_list.left() >= navigation.right());
    assert!(reader.left() >= message_list.right());
    assert!(reader.size.width > px(0.));
}

#[gpui_kit::test]
fn unavailable_calendar_navigation_explains_why_it_is_disabled(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx
        .add_window_view(|_, cx| AccountView::new(Ok(AccountServices::new(workflow, factory)), cx));
    let profile = mail_only_profile();
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(connected_account(profile));
            cx.notify();
        });
    });
    context.run_until_parked();
    draw_account_view(context);
    assert!(context.debug_bounds("nav-calendar").is_some());
    assert_eq!(
        context.cx.update(|cx| {
            view.read(cx)
                .navigation_disabled_reason(WorkspaceSection::Calendar, cx)
        }),
        Some("Calendar is unavailable for this account.")
    );
    click(context, "nav-calendar");
    assert!(
        context.cx.update(|cx| {
            view.read(cx).content.selected_section() == WorkspaceSection::Accounts
        })
    );
}

type WorkspaceTestView = gpui_kit::Entity<AccountView<TestProvider>>;
fn assert_workspace_navigation_is_visible(context: &mut VisualTestContext) {
    for selector in ["nav-mail", "nav-calendar", "nav-accounts"] {
        assert!(context.debug_bounds(selector).is_some());
    }
}

fn assert_disconnected_navigation_explains_disabled_destinations(
    view: &WorkspaceTestView,
    context: &mut VisualTestContext,
) {
    let reasons = context.cx.update(|cx| {
        let view = view.read(cx);
        [
            view.navigation_disabled_reason(WorkspaceSection::Mail, cx),
            view.navigation_disabled_reason(WorkspaceSection::Calendar, cx),
        ]
    });
    assert_eq!(
        reasons,
        [
            Some("Connect an account to use this section."),
            Some("Connect an account to use this section."),
        ]
    );
}

fn assert_disconnected_workspace_routes(view: &WorkspaceTestView, context: &mut VisualTestContext) {
    context.run_until_parked();
    draw_account_view(context);
    assert_workspace_navigation_is_visible(context);
    assert_disconnected_navigation_explains_disabled_destinations(view, context);
    click(context, "nav-mail");
    assert!(context.cx.update(|cx| {
        matches!(&view.read(cx).content, AccountContent::Account)
            && matches!(&view.read(cx).status, AccountStatus::Disconnected)
    }));
}

fn assert_keyboard_navigation_selects_calendar(
    view: &WorkspaceTestView,
    context: &mut VisualTestContext,
) {
    draw_account_view(context);
    focus_calendar_after_mail(view, context);
    press_enter(context);
    draw_account_view(context);
    assert!(
        context.cx.update(|cx| {
            view.read(cx).content.selected_section() == WorkspaceSection::Calendar
        })
    );
}

fn connect_synthetic_workspace_account(view: &WorkspaceTestView, context: &mut VisualTestContext) {
    let profile = AccountProfile::try_new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
    )
    .expect("synthetic account profile is valid");
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.status = AccountStatus::Connected(connected_account(profile));
            cx.notify();
        });
    });
}

fn focus_calendar_after_mail(view: &WorkspaceTestView, context: &mut VisualTestContext) {
    let mail_focus = context
        .cx
        .update(|cx| view.read(cx).mail_navigation_focus.clone());
    context.update(|window, cx| {
        mail_focus.focus(window, cx);
        window.focus_next(cx);
    });
    let calendar_focus = context
        .cx
        .update(|cx| view.read(cx).calendar_navigation_focus.clone());
    assert!(
        context.update(|window, _| calendar_focus.is_focused(window)),
        "Tab reaches the Calendar navigation button"
    );
}

fn press_enter(context: &mut VisualTestContext) {
    let keystroke = gpui_kit::Keystroke::parse("enter").expect("Enter keystroke is valid");
    context.simulate_event(gpui_kit::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    context.simulate_event(gpui_kit::KeyUpEvent { keystroke });
    context.run_until_parked();
}

#[test]
fn opaque_navigation_keeps_the_theme_sidebar_unchanged() {
    let sidebar = Background::from(Hsla {
        h: 0.62,
        s: 0.34,
        l: 0.31,
        a: 1.0,
    });

    assert_eq!(
        NavigationSurfaceAppearance::Opaque.navigation_background(sidebar),
        sidebar
    );
}

#[test]
fn translucent_navigation_applies_alpha_to_the_theme_sidebar() {
    let sidebar = Background::from(Hsla {
        h: 0.62,
        s: 0.34,
        l: 0.31,
        a: 1.0,
    });

    assert_eq!(
        NavigationSurfaceAppearance::Translucent.navigation_background(sidebar),
        sidebar.opacity(super::super::NAVIGATION_SURFACE_OPACITY)
    );
}
