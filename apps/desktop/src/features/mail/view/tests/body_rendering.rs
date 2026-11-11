use super::*;

#[gpui_kit::test]
fn cached_html_mail_view_sanitizes_content_and_opens_links_on_click(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
        MessageBodyType::Html,
        concat!(
            "<p><a href=\"https://mail.example.test/thread\">Open safe link</a></p>",
            "<p>Readable synthetic message content.</p>",
            "<img src=\"https://tracker.example.test/pixel\">",
            "<script>hidden script marker</script>"
        )
        .to_owned(),
    ));
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
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    let safe_html = context.cx.update(|cx| {
        let state = &view.read(cx).message_detail_state;
        let MessageDetailState::Opened(opened) = state else {
            return false;
        };
        let MessageBodyForDisplay::SanitizedHtml(html) = opened.body() else {
            return false;
        };
        html.as_str().contains("Open safe link")
            && !html.as_str().contains("tracker.example.test")
            && !html.as_str().contains("hidden script marker")
    });
    assert!(safe_html);

    let body_bounds = context
        .debug_bounds("mail-message-html-content-region")
        .expect("sanitized HTML body has a layout");
    let link_position = point(body_bounds.left() + px(36.), body_bounds.top() + px(36.));
    context.simulate_mouse_down(link_position, MouseButton::Right, Modifiers::default());
    context.simulate_mouse_up(link_position, MouseButton::Right, Modifiers::default());
    assert_eq!(context.opened_url(), None);
    context.simulate_click(link_position, Modifiers::default());
    assert_eq!(
        context.opened_url(),
        Some("https://mail.example.test/thread".to_owned())
    );
}

#[gpui_kit::test]
fn cached_html_links_can_be_opened_from_the_keyboard_accessible_menu(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let body = concat!(
        "<p><a href=\"https://mail.example.test/private?tracking=marker\">Open safe link</a></p>",
        "<p><a href=\"https://user%3Avalue@mail.example.test/account\">Userinfo link</a></p>",
        "<svg><a href=\"https://hidden.example.test/removed\">Removed SVG link</a></svg>"
    )
    .to_owned();
    let context = open_cached_html_message(cx, body);

    let mut links_button_focused = false;
    for _ in 0..32 {
        context.update(gpui_kit::Window::focus_next);
        draw_mail_view(context);
        links_button_focused = context.update(|window, _| {
            window
                .find_all_by_role(gpui_kit::Role::Button)
                .iter()
                .any(|button| {
                    button.label() == Some("Message links") && button.focused() == Some(true)
                })
        });
        if links_button_focused {
            break;
        }
    }
    let button_states = context.update(|window, _| {
        window
            .find_all_by_role(gpui_kit::Role::Button)
            .iter()
            .map(|button| (button.label().map(str::to_owned), button.focused()))
            .collect::<Vec<_>>()
    });
    assert!(
        links_button_focused,
        "focus traversal reaches the message links button: {button_states:?}"
    );
    context.simulate_keystrokes("enter");
    draw_mail_view(context);

    let menu_labels = context.update(|window, _| {
        window
            .find_all_by_role(gpui_kit::Role::MenuItem)
            .iter()
            .map(|item| item.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert_eq!(menu_labels, [Some("Link 1 — mail.example.test".to_owned())]);
    assert!(
        !menu_labels
            .iter()
            .flatten()
            .any(|label| label.contains("private") || label.contains("tracking=")),
        "menu labels must not expose URL paths or queries"
    );

    context.simulate_keystrokes("down");
    context.simulate_keystrokes("enter");
    assert_eq!(
        context.opened_url(),
        Some("https://mail.example.test/private?tracking=marker".to_owned())
    );
}

#[gpui_kit::test]
fn cached_html_links_menu_reports_when_safe_links_are_omitted(cx: &mut TestAppContext) {
    const SAFE_LINK_LIMIT: usize = 100;

    cx.update(gpui_kit::init);
    let body = (0..=SAFE_LINK_LIMIT)
        .map(|index| {
            format!("<a href=\"https://link-{index}.example.test/private?tracking=marker-{index}\">link</a>")
        })
        .collect::<String>();
    let context = open_cached_html_message(cx, body);
    let links_button = context
        .debug_bounds("mail-message-links")
        .expect("safe message links have a menu");
    context.simulate_click(links_button.center(), Modifiers::default());
    draw_mail_view(context);

    let menu_labels = context.update(|window, _| {
        window
            .find_all_by_role(gpui_kit::Role::MenuItem)
            .iter()
            .map(|item| item.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert_eq!(menu_labels.len(), SAFE_LINK_LIMIT + 1);
    assert_eq!(
        menu_labels.last(),
        Some(&Some("Additional links omitted.".to_owned()))
    );
    assert_eq!(
        menu_labels
            .iter()
            .flatten()
            .filter(|label| label.starts_with("Link "))
            .count(),
        SAFE_LINK_LIMIT
    );
    assert!(
        menu_labels
            .iter()
            .flatten()
            .all(|label| { !label.contains("private") && !label.contains("tracking=") })
    );
}

#[gpui_kit::test]
fn cached_html_mail_view_does_not_open_unsafe_links(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
        MessageBodyType::Html,
        concat!(
            "<p><a href=\"javascript:alert(1)\">Unsafe synthetic link</a></p>",
            "<img src=\"https://tracker.example.test/pixel\">",
            "<script>hidden script marker</script>"
        )
        .to_owned(),
    ));
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
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    let sanitized_body = context.cx.update(|cx| {
        let MessageDetailState::Opened(opened) = &view.read(cx).message_detail_state else {
            return false;
        };
        let MessageBodyForDisplay::SanitizedHtml(html) = opened.body() else {
            return false;
        };
        !html.as_str().contains("href=")
            && !html.as_str().contains("tracker.example.test")
            && !html.as_str().contains("hidden script marker")
            && html.as_str().contains("Unsafe synthetic link")
    });
    assert!(sanitized_body);
    assert!(
        context.debug_bounds("mail-message-links").is_none(),
        "unsafe links do not create a message-links menu"
    );

    let body_bounds = context
        .debug_bounds("mail-message-html-content-region")
        .expect("sanitized HTML body has a layout");
    context.simulate_click(
        point(body_bounds.left() + px(36.), body_bounds.top() + px(36.)),
        Modifiers::default(),
    );
    assert_eq!(context.opened_url(), None);
}

#[gpui_kit::test]
fn cached_html_mail_view_supports_selection_and_copy(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
        MessageBodyType::Html,
        "<p>Readable synthetic content for selection and copy.</p>".to_owned(),
    ));
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (_, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    context.run_until_parked();
    draw_mail_view(context);
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    let body_bounds = context
        .debug_bounds("mail-message-html-content-region")
        .expect("sanitized HTML body has a layout");
    let start = point(body_bounds.left() + px(25.), body_bounds.top() + px(34.));
    let end = point(body_bounds.left() + px(285.), body_bounds.top() + px(34.));
    context.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    draw_mail_view(context);
    context.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    draw_mail_view(context);
    context.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    draw_mail_view(context);

    let selected_text = context.update(TextSelection::selected_text);
    assert!(selected_text.contains("Readable synthetic content"));
    context.simulate_keystrokes("ctrl-c");
    let copied_text = context.update(|_, app| {
        app.read_from_clipboard()
            .and_then(|clipboard| clipboard.text())
    });
    assert_eq!(copied_text, Some(selected_text.trim_end().to_owned()));
}

#[gpui_kit::test]
fn oversized_sanitized_html_shows_selectable_noninteractive_text(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let body_content = format!(
        "<p>Readable fallback content. <a href=\"https://mail.example.test/path\">link destination</a> {}</p>",
        ">".repeat(8 * 1024 * 1024 / 4)
    );
    cache.0.lock().expect("fixture cache mutex is healthy").body =
        Some(MessageBody::new(MessageBodyType::Html, body_content));
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
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    let status = context
        .debug_bounds("mail-message-html-fallback-status")
        .expect("plain-text fallback explains why formatting was removed");
    assert!(status.size.width > px(0.));
    let body_bounds = context
        .debug_bounds("mail-message-html-fallback-body")
        .expect("plain-text fallback has a selectable content region");

    let contains_fallback_text = context.cx.update(|cx| {
        matches!(
            &view.read(cx).message_detail_state,
            MessageDetailState::Opened(opened)
                if matches!(
                    opened.body(),
                    MessageBodyForDisplay::PlainTextFallback(content)
                        if content.as_str().contains("Readable&#32;fallback&#32;content")
                            && content.as_str().contains("link&#32;destination")
                )
        )
    });
    assert!(contains_fallback_text);

    context.simulate_click(
        point(body_bounds.left() + px(280.), body_bounds.top() + px(10.)),
        Modifiers::default(),
    );
    assert_eq!(context.opened_url(), None);
    context.simulate_keystrokes("ctrl-a");

    let selected_text = context.update(TextSelection::selected_text);
    assert!(selected_text.contains("Readable fallback content"));
    context.simulate_keystrokes("ctrl-c");
    let copied_text = context.update(|_, app| {
        app.read_from_clipboard()
            .and_then(|clipboard| clipboard.text())
    });
    assert_eq!(copied_text, Some(selected_text.trim_end().to_owned()));
}

#[gpui_kit::test]
fn oversized_cached_html_shows_a_safe_reader_fallback(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
        MessageBodyType::Html,
        "x".repeat(8 * 1024 * 1024 + 1),
    ));
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
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    let is_over_limit = context.cx.update(|cx| {
        matches!(
            view.read(cx).message_detail_state,
            MessageDetailState::Opened(ref opened)
                if opened.body() == &MessageBodyForDisplay::HtmlTooLarge
        )
    });
    assert!(is_over_limit);
    assert!(
        context
            .debug_bounds("mail-message-body-too-large")
            .is_some()
    );
}
