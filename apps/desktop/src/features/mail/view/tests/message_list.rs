use super::*;

#[gpui_kit::test]
async fn cached_message_list_navigates_by_keyboard_and_keeps_one_tab_stop(cx: &mut TestAppContext) {
    const FIRST_MESSAGE_LABEL: &str = SYNTHETIC_MESSAGE_LABEL_PREFIX;
    const SECOND_MESSAGE_LABEL: &str = "Ada Example <ada@example.test>. Synthetic subject 2";
    const LAST_MESSAGE_LABEL: &str = "Ada Example <ada@example.test>. Synthetic subject 20";

    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_message_navigation_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let message_ids = append_synthetic_messages(&cache, 20);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
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

    assert_accessible_message_list(context);
    assert_eq!(accessible_message_list_focus(context), Some(false));

    context.update(gpui_kit::Window::focus_next);
    let mut list_focused = false;
    for _ in 0..20 {
        press_window_key(context, "tab");
        list_focused =
            context.update(|window, cx| view.read(cx).message_list_focus.is_focused(window));
        if list_focused {
            break;
        }
    }
    assert!(list_focused, "Tab reaches the cached message list");
    assert_eq!(accessible_message_list_focus(context), Some(true));

    press_window_key(context, "up");
    context.run_until_parked();
    draw_mail_view(context);
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.last(),
        LAST_MESSAGE_LABEL,
    );
    press_window_key(context, "down");
    context.run_until_parked();
    draw_mail_view(context);
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.last(),
        LAST_MESSAGE_LABEL,
    );

    press_window_key(context, "home");
    context.run_until_parked();
    draw_mail_view(context);
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.first(),
        FIRST_MESSAGE_LABEL,
    );
    press_window_key(context, "up");
    context.run_until_parked();
    draw_mail_view(context);
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.first(),
        FIRST_MESSAGE_LABEL,
    );
    press_window_key(context, "down");
    context.run_until_parked();
    draw_mail_view(context);
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.get(1),
        SECOND_MESSAGE_LABEL,
    );
    press_window_key(context, "end");
    context.run_until_parked();
    draw_mail_view(context);
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.last(),
        LAST_MESSAGE_LABEL,
    );
    assert!(context.cx.update(|cx| matches!(
        &view.read(cx).message_detail_state,
        MessageDetailState::Opened(_)
    )));
    let row = context
        .debug_bounds("message-row-message-20")
        .expect("End scrolls the selected message into view");
    let list_bounds = context.update(|window, _| window.find("cached-message-list").bounds());
    assert!(
        row.top() >= list_bounds.top() && row.bottom() <= list_bounds.bottom(),
        "End keeps the selected message inside the list viewport"
    );
    let scroll_offset = context
        .cx
        .update(|cx| view.read(cx).message_list_scroll.offset());
    assert!(scroll_offset.y < px(0.));
    context.cx.update(|cx| {
        view.update(cx, |view, _| {
            view.message_list_scroll
                .scroll_to_item(0, ScrollStrategy::Top);
        });
    });
    context.run_until_parked();
    draw_mail_view(context);
    let first_row = context
        .debug_bounds("message-row-message-1")
        .expect("the first message is visible after scrolling to the top");
    context.simulate_click(first_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.first(),
        FIRST_MESSAGE_LABEL,
    );
    assert!(context.update(|window, cx| { view.read(cx).message_list_focus.is_focused(window) }));
    assert_eq!(accessible_message_list_focus(context), Some(true));

    press_window_key(context, "shift-tab");
    context.update(|window, cx| {
        assert!(!view.read(cx).message_list_focus.is_focused(window));
    });
    press_window_key(context, "up");
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.first(),
        FIRST_MESSAGE_LABEL,
    );
    press_window_key(context, "tab");
    assert!(context.update(|window, cx| { view.read(cx).message_list_focus.is_focused(window) }));
    assert_eq!(accessible_message_list_focus(context), Some(true));
    press_window_key(context, "tab");
    context.update(|window, cx| {
        assert!(!view.read(cx).message_list_focus.is_focused(window));
    });
    assert_eq!(accessible_message_list_focus(context), Some(false));
    press_window_key(context, "up");
    assert_selected_message_and_accessible_label(
        context,
        &view,
        message_ids.first(),
        FIRST_MESSAGE_LABEL,
    );
}

#[gpui_kit::test]
async fn message_list_width_controls_are_focusable_and_bounded(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_message_navigation_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let provider = OfflineProvider {
        body_fetches: Arc::new(AtomicUsize::new(0)),
    };
    let body_fetches = Arc::clone(&provider.body_fetches);
    let workflow = Arc::new(MailWorkflow::new(profile.id().clone(), provider, cache));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    context.simulate_resize(size(px(1600.), px(900.)));
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);

    assert_eq!(message_list_width(context), px(352.));
    context.update(|_, app| {
        view.update(app, |mail_view, cx| {
            mail_view.select_message(message_id.clone(), cx)
        });
    });
    wait_for_ui_condition(context, |_, app| {
        matches!(
            &view.read(app).message_detail_state,
            MessageDetailState::Opened(_)
        )
    })
    .await;
    context.run_until_parked();
    draw_mail_view(context);
    assert!(context.cx.update(|cx| {
        matches!(
            &view.read(cx).message_detail_state,
            MessageDetailState::Opened(_)
        ) && view.read(cx).selected_message_id() == Some(&message_id)
    }));

    context.update(gpui_kit::Window::focus_next);
    let mut narrower_focused = false;
    for _ in 0..20 {
        narrower_focused =
            context.update(|window, _| window.find_by_label("Narrow list").focused() == Some(true));
        if narrower_focused {
            break;
        }
        press_window_key(context, "tab");
    }
    assert!(narrower_focused, "Tab reaches the Narrow list button");
    assert_eq!(
        context.update(|window, _| window.find_by_label("Narrow list").role()),
        Some(gpui_kit::Role::Button)
    );

    activate_focused_control(context);
    context.run_until_parked();
    draw_mail_view(context);
    assert_eq!(message_list_width(context), px(336.));

    press_window_key(context, "tab");
    assert_eq!(
        context.update(|window, _| window.find_by_label("Widen list").focused()),
        Some(true)
    );
    assert_eq!(
        context.update(|window, _| window.find_by_label("Widen list").role()),
        Some(gpui_kit::Role::Button)
    );
    activate_focused_control(context);
    context.run_until_parked();
    draw_mail_view(context);
    assert_eq!(message_list_width(context), px(352.));

    for _ in 0..4 {
        let position =
            context.update(|window, _| window.find_by_label("Narrow list").bounds().center());
        context.simulate_click(position, Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
    }
    assert_eq!(message_list_width(context), px(288.));
    assert_eq!(
        context.update(|window, _| window.find_by_label("Narrow list").disabled()),
        Some(true)
    );

    let position =
        context.update(|window, _| window.find_by_label("Narrow list").bounds().center());
    context.simulate_click(position, Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    assert_eq!(message_list_width(context), px(288.));

    for _ in 0..12 {
        let position =
            context.update(|window, _| window.find_by_label("Widen list").bounds().center());
        context.simulate_click(position, Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
    }
    assert_eq!(message_list_width(context), px(480.));
    assert_eq!(
        context.update(|window, _| window.find_by_label("Widen list").disabled()),
        Some(true)
    );
    assert!(
        context.update(|window, _| { window.find("mail-reader").bounds().size.width >= px(480.) })
    );
    assert!(context.cx.update(|cx| {
        matches!(
            &view.read(cx).message_detail_state,
            MessageDetailState::Opened(_)
        ) && view.read(cx).selected_message_id() == Some(&message_id)
    }));
    assert_eq!(body_fetches.load(Ordering::Relaxed), 0);
}

#[gpui_kit::test]
async fn compact_mail_keeps_folder_context_and_supports_pointer_and_keyboard_navigation(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_message_navigation_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
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
    let view = mail_view.expect("mail view is created with the test window");

    context.simulate_resize(size(px(900.), px(720.)));
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);

    let (viewport, folder_navigation, inbox_folder, message_list) = context.update(|window, _| {
        (
            window.bounds(),
            window.find("mail-folder-navigation").bounds(),
            window.find("folder:inbox"),
            window.find("mail-message-list").bounds(),
        )
    });
    assert!(folder_navigation.right() <= viewport.right());
    assert!(message_list.right() <= viewport.right());
    assert_eq!(inbox_folder.label(), Some("Inbox"));
    assert!(context.update(|window, _| window.try_find_by_label("Widen list").is_none()));
    assert!(context.cx.update(|app| {
        view.read(app)
            .selected_folder
            .as_ref()
            .is_some_and(|folder| folder.folder_id().as_str() == "inbox")
    }));
    assert!(context.update(|window, _| window.try_find("mail-reader").is_none()));
    assert_accessible_message_list(context);

    context.update(|window, app| window.click("message-option-message-1", app));
    context.run_until_parked();
    draw_mail_view(context);

    assert!(
        context
            .cx
            .update(|app| matches!(view.read(app).compact_pane, CompactMailPane::MessageReader))
    );
    assert!(context.update(|window, _| window.try_find("cached-message-list").is_none()));
    assert!(
        context
            .update(|window, _| { window.find("mail-reader").label() == Some("Message reader") })
    );
    assert!(context.update(|window, app| {
        window.find_by_label("Back to messages").visible()
            && view.read(app).message_list_focus.is_focused(window)
    }));
    assert!(context.cx.update(|app| {
        view.read(app).selected_message_id() == Some(&message_id)
            && matches!(
                &view.read(app).message_detail_state,
                MessageDetailState::Opened(_)
            )
    }));

    let back_button =
        context.update(|window, _| window.find_by_label("Back to messages").bounds().center());
    context.simulate_click(back_button, Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    assert!(context.update(|window, app| {
        window.try_find("cached-message-list").is_some()
            && window.try_find("mail-reader").is_none()
            && view.read(app).message_list_focus.is_focused(window)
    }));
    assert_selected_message_and_accessible_label(
        context,
        &view,
        Some(&message_id),
        SYNTHETIC_MESSAGE_LABEL_PREFIX,
    );

    press_window_key(context, "enter");
    draw_mail_view(context);
    assert!(context.update(|window, _| window.try_find("mail-reader").is_some()));
    press_window_key(context, "escape");
    draw_mail_view(context);
    assert!(context.update(|window, app| {
        window.try_find("cached-message-list").is_some()
            && window.try_find("mail-reader").is_none()
            && view.read(app).message_list_focus.is_focused(window)
    }));

    context.simulate_resize(size(px(1600.), px(900.)));
    draw_mail_view(context);
    assert!(context.update(|window, app| {
        window.find("mail-message-list").bounds().size.width > px(0.)
            && window.find("mail-reader").bounds().size.width > px(0.)
            && view.read(app).selected_message_id() == Some(&message_id)
    }));

    context.simulate_resize(size(px(900.), px(720.)));
    draw_mail_view(context);
    assert!(context.update(|window, app| {
        window.try_find("cached-message-list").is_some()
            && window.try_find("mail-reader").is_none()
            && view.read(app).selected_message_id() == Some(&message_id)
    }));
    assert_selected_message_and_accessible_label(
        context,
        &view,
        Some(&message_id),
        SYNTHETIC_MESSAGE_LABEL_PREFIX,
    );

    context.simulate_resize(size(px(1600.), px(900.)));
    draw_mail_view(context);
    for _ in 0..8 {
        let position =
            context.update(|window, _| window.find_by_label("Widen list").bounds().center());
        context.simulate_click(position, Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);
    }
    assert_eq!(message_list_width(context), px(480.));

    context.simulate_resize(size(px(1200.), px(720.)));
    draw_mail_view(context);
    assert!(context.update(|window, _| {
        window.try_find("cached-message-list").is_some() && window.try_find("mail-reader").is_none()
    }));

    context.simulate_resize(size(px(1260.), px(720.)));
    draw_mail_view(context);
    assert!(context.update(|window, _| {
        window.find("mail-message-list").bounds().size.width > px(0.)
            && window.find("mail-reader").bounds().size.width >= px(480.)
    }));
}

#[gpui_kit::test]
async fn dragged_message_list_width_keeps_its_rem_size_after_zoom(cx: &mut TestAppContext) {
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
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");

    context.simulate_resize(size(px(1600.), px(900.)));
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);
    let handle = context.update(|window, _| window.find(("resizable-handle", 0usize)).bounds());
    let start = handle.center();
    let end = point(start.x + px(32.), start.y);
    context.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    context.simulate_mouse_move(
        point(start.x + px(10.), start.y),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    context.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    context.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    assert_eq!(message_list_width(context), px(384.5));
    assert_eq!(
        context.cx.update(|cx| {
            view.read(cx)
                .pane_layout
                .message_list_width
                .to_pixels(px(16.))
        }),
        px(384.5)
    );
    context.cx.update(|cx| {
        gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(24.));
    });
    context.simulate_resize(size(px(1800.), px(900.)));
    context.run_until_parked();
    draw_mail_view(context);

    assert_eq!(
        context.cx.update(|cx| {
            view.read(cx)
                .pane_layout
                .message_list_width
                .to_pixels(px(24.))
        }),
        px(576.75)
    );
    let scaled_width = message_list_width(context);
    assert!(
        (px(576.5)..=px(577.)).contains(&scaled_width),
        "rem-based width scales with the window zoom: {scaled_width:?}"
    );
}

#[gpui_kit::test]
async fn cached_message_list_rescales_rows_and_virtual_scroll_with_rem(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_message_navigation_keys(cx);
    });
    let account = synthetic_account_id();
    let (profile, cache, _) = fixture(account);
    let message_ids = append_synthetic_messages(&cache, 20);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
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
    draw_mail_view(context);

    let first_row = context
        .debug_bounds("message-row-message-1")
        .expect("the first cached message is visible");
    assert_eq!(first_row.size.height, px(72.));
    let list_bounds = context.update(|window, _| window.find("cached-message-list").bounds());
    assert_eq!(list_bounds.size.width, px(351.));

    context.cx.update(|cx| {
        gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(24.));
    });
    context.run_until_parked();
    draw_mail_view(context);

    let scaled_row = context
        .debug_bounds("message-row-message-1")
        .expect("the first cached message remains visible after zoom");
    assert_eq!(scaled_row.size.height, px(108.));
    let scaled_row_estimate = context
        .cx
        .update(|cx| view.read(cx).message_row_sizes[0].height);
    assert_eq!(
        scaled_row_estimate,
        px(150.),
        "the first item is a 108px row plus the 42px day header"
    );
    let list_bounds = context.update(|window, _| window.find("cached-message-list").bounds());
    assert_eq!(list_bounds.size.width, px(527.));

    context.cx.update(|cx| {
        view.update(cx, |view, _| {
            view.message_list_scroll
                .scroll_to_item(message_ids.len() - 1, ScrollStrategy::Top);
        });
    });
    context.run_until_parked();
    draw_mail_view(context);

    let last_row = context
        .debug_bounds("message-row-message-20")
        .expect("the final message is rendered after scrolling at the new zoom");
    let list_bounds = context.update(|window, _| window.find("cached-message-list").bounds());
    assert!(
        last_row.top() >= list_bounds.top() && last_row.bottom() <= list_bounds.bottom(),
        "virtual scrolling uses row estimates at the current rem size"
    );
}
