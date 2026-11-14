use einfach_mail_domain::{MailFolder, UnreadCount};

use super::*;
use crate::test_support::mail::FixtureCache;

/// Gives the fixture's Inbox and Archive folders their kinds and unread counts, and returns the
/// folders a provider listing would report.
fn count_fixture_folders(cache: &FixtureCache, inbox: u32, archive: u32) -> Vec<MailFolder> {
    let mut data = cache.0.lock().expect("fixture cache mutex is healthy");
    data.folders = data
        .folders
        .drain(..)
        .map(|folder| {
            let (kind, count) = match folder.display_name() {
                "Inbox" => (MailFolderKind::Inbox, inbox),
                _ => (MailFolderKind::Archive, archive),
            };
            folder
                .with_kind(Some(kind))
                .with_unread_count(Some(UnreadCount::new(count)))
        })
        .collect();
    data.folders.clone()
}

fn folder_button_exists(context: &mut VisualTestContext, label: &str) -> bool {
    context.update(|window, _| window.try_find_by_label(label).is_some())
}

fn folder_unread_counts(context: &mut VisualTestContext, view: &Entity<MailView>) -> Vec<u32> {
    context.cx.update(|cx| {
        view.read(cx)
            .folders
            .iter()
            .map(|folder| folder.unread_count().map_or(0, UnreadCount::get))
            .collect()
    })
}

#[gpui_kit::test]
async fn folders_show_their_unread_counts_in_the_accessible_name(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (profile, cache, _) = fixture(synthetic_account_id());
    let folders = count_fixture_folders(&cache, 12, 0);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Confirm,
            Arc::new(Mutex::new(Vec::new())),
        )
        .with_folders(folders),
        cache,
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);

    assert!(folder_button_exists(context, "Inbox, 12 unread"));
    assert!(
        folder_button_exists(context, "Archive"),
        "a folder with no unread mail keeps its plain name"
    );
    assert!(context.debug_bounds("folder:inbox-unread").is_some());
    assert!(context.debug_bounds("folder:archive-unread").is_none());
}

#[gpui_kit::test]
async fn marking_a_message_read_lowers_the_folder_unread_count(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (profile, cache, _) = fixture(synthetic_account_id());
    let folders = count_fixture_folders(&cache, 5, 2);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Confirm,
            Arc::new(Mutex::new(Vec::new())),
        )
        .with_folders(folders),
        cache,
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);
    assert!(folder_button_exists(context, "Inbox, 5 unread"));

    open_cached_message_and_click_read_state(context);

    assert_eq!(folder_unread_counts(context, &view), [4, 2]);
    assert!(folder_button_exists(context, "Inbox, 4 unread"));
    assert!(!folder_button_exists(context, "Inbox, 5 unread"));
    assert!(folder_button_exists(context, "Archive, 2 unread"));
}

#[gpui_kit::test]
async fn moving_an_unread_message_moves_one_unread_between_the_folder_counts(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let (profile, cache, message_id) = fixture(synthetic_account_id());
    count_fixture_folders(&cache, 1, 0);
    let destination = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .folders
        .iter()
        .find(|folder| folder.display_name() == "Archive")
        .expect("fixture cache contains an archive folder")
        .reference()
        .clone();
    let provider = message_move_provider(
        &cache,
        MessageMoveActionResponse::Confirm,
        Arc::new(AtomicUsize::new(0)),
    );
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        provider,
        cache.clone(),
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);
    assert!(folder_button_exists(context, "Inbox, 1 unread"));
    let row = context
        .debug_bounds("message-row-message-1")
        .expect("source message is visible in the cached inbox");
    context.simulate_click(row.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "message-actions")
    })
    .await;

    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.apply_message_move_menu_action(
                MessageMoveMenuAction {
                    selected_message_id: message_id.clone(),
                    destination_folder: destination.clone(),
                    destination_label: "Archive".to_owned(),
                },
                cx,
            );
        });
    });
    wait_for_ui_condition(context, |_, app| {
        matches!(
            view.read(app).message_detail_state,
            MessageDetailState::MoveCompleted { .. }
        )
    })
    .await;
    draw_mail_view(context);

    assert!(folder_button_exists(context, "Inbox"));
    assert!(folder_button_exists(context, "Archive, 1 unread"));
}

#[gpui_kit::test]
async fn pinned_folders_lead_the_navigation_and_tab_follows_that_order(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (profile, cache, _) = fixture(synthetic_account_id());
    let mut folders = count_fixture_folders(&cache, 5, 0);
    folders.reverse();
    let custom = MailFolder::new(
        FolderReference::new(
            profile.id().clone(),
            FolderId::try_from("projects".to_owned()).expect("synthetic folder ID is valid"),
        ),
        None,
        "Projects".to_owned(),
    )
    .expect("synthetic folder is valid");
    folders.insert(0, custom);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(
            ReadStateActionResponse::Confirm,
            Arc::new(Mutex::new(Vec::new())),
        )
        .with_folders(folders),
        cache,
    ));
    let mut mail_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let (root, view) = test_mail_workspace(profile, workflow, window, cx);
        mail_view = Some(view);
        root
    });
    let view = mail_view.expect("mail view is attached to contextual folder navigation");
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);

    let tops: Vec<_> = ["folder:inbox", "folder:archive", "folder:projects"]
        .into_iter()
        .map(|selector| {
            context
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} is rendered"))
                .top()
        })
        .collect();
    assert!(
        tops[0] < tops[1] && tops[1] < tops[2],
        "Inbox, then Archive, then the unpinned folder: {tops:?}"
    );
    context.update(gpui_kit::Window::focus_next);
    for expected in ["New mail", "Inbox, 5 unread", "Archive", "Projects"] {
        assert_eq!(
            context.update(|window, _| window.find_by_label(expected).focused()),
            Some(true),
            "Tab reaches {expected}"
        );
        press_window_key(context, "tab");
    }
}
