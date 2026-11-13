use super::*;
use crate::components::NAVIGATION_PANE_WIDTH;

use super::super::MESSAGE_LIST_DEFAULT_WIDTH;

const MORE_ACTIONS: &str = "message-actions-more";
const WINDOW_HEIGHT: f32 = 800.;

fn open_message_at_width(
    cx: &mut TestAppContext,
    window_width: gpui_kit::Pixels,
) -> (Entity<MailView>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (profile, cache, _) = fixture(synthetic_account_id());
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    context.simulate_resize(size(window_width, px(WINDOW_HEIGHT)));
    open_first_cached_message(context);
    (view, context)
}

fn reader_width_threshold(context: &mut VisualTestContext, compact: bool) -> gpui_kit::Pixels {
    let rem_size = context.update(|window, _| window.rem_size());
    let list_width = if compact {
        px(0.)
    } else {
        MESSAGE_LIST_DEFAULT_WIDTH.to_pixels(rem_size)
    };
    NAVIGATION_PANE_WIDTH.to_pixels(rem_size) + list_width + gpui_kit::rems(40.).to_pixels(rem_size)
}

fn has_expanded_secondary_commands(context: &mut VisualTestContext) -> bool {
    context.debug_bounds("message-read-state").is_some()
        && context.debug_bounds("message-follow-up-flag").is_some()
}

#[gpui_kit::test]
fn beside_the_list_the_bar_folds_secondary_commands_below_forty_rem(cx: &mut TestAppContext) {
    let (_, context) = open_message_at_width(cx, px(1280.));
    let threshold = reader_width_threshold(context, false);
    assert!(context.debug_bounds("message-reply").is_some());
    assert!(has_expanded_secondary_commands(context));
    assert!(context.debug_bounds(MORE_ACTIONS).is_none());

    context.simulate_resize(size(threshold, px(WINDOW_HEIGHT)));
    draw_mail_view(context);
    assert!(
        has_expanded_secondary_commands(context),
        "a reader of exactly 40rem keeps every command visible"
    );
    assert!(context.debug_bounds(MORE_ACTIONS).is_none());

    context.simulate_resize(size(threshold - px(1.), px(WINDOW_HEIGHT)));
    draw_mail_view(context);
    for primary in [
        "message-reply",
        "message-archive",
        "message-move",
        "message-delete",
    ] {
        assert!(
            context.debug_bounds(primary).is_some(),
            "{primary} stays on the bar"
        );
    }
    assert!(context.debug_bounds("message-read-state").is_none());
    assert!(context.debug_bounds("message-follow-up-flag").is_none());
    assert!(context.debug_bounds(MORE_ACTIONS).is_some());
}

#[gpui_kit::test]
fn in_compact_mode_the_bar_counts_the_whole_width_beside_the_navigation_pane(
    cx: &mut TestAppContext,
) {
    let (_, context) = open_message_at_width(cx, px(1000.));
    let threshold = reader_width_threshold(context, true);
    assert!(
        context.update(|window, _| window.try_find_by_label("Back to messages").is_some()),
        "1000px is the compact layout"
    );
    assert!(
        threshold < px(1000.),
        "the compact threshold lies inside the compact layout"
    );
    assert!(has_expanded_secondary_commands(context));

    context.simulate_resize(size(threshold - px(1.), px(WINDOW_HEIGHT)));
    draw_mail_view(context);
    assert!(context.debug_bounds("message-read-state").is_none());
    assert!(context.debug_bounds(MORE_ACTIONS).is_some());
}

#[gpui_kit::test]
fn the_compact_back_button_leads_the_command_bar_on_one_row(cx: &mut TestAppContext) {
    let (_, context) = open_message_at_width(cx, px(1000.));

    let (toolbar, back, bar) = context.update(|window, _| {
        (
            window.find("mail-compact-reader-toolbar").bounds(),
            window.find_by_label("Back to messages").bounds(),
            window.find("message-actions").bounds(),
        )
    });
    assert!(back.right() <= bar.left(), "the back button leads the bar");
    assert!(
        (back.center().y - bar.center().y).abs() < px(8.),
        "the back button and the bar share one row"
    );
    assert!(toolbar.contains(&back.center()) && toolbar.contains(&bar.center()));
    assert_eq!(
        context.update(|window, _| window.find_all_by_role(gpui_kit::Role::Toolbar).len()),
        1,
        "the bar is rendered once"
    );
}

#[gpui_kit::test]
fn the_split_reader_puts_the_bar_above_the_subject_and_sender(cx: &mut TestAppContext) {
    let (_, context) = open_message_at_width(cx, px(1280.));

    let (reader, bar) = context.update(|window, _| {
        (
            window.find("mail-reader").bounds(),
            window.find("message-actions").bounds(),
        )
    });
    assert!(
        bar.top() - reader.top() < px(16.),
        "the bar opens the reader, above the subject"
    );
    assert!(
        context.update(|window, _| window.try_find_by_label("Back to messages").is_none()),
        "the split layout has no back button"
    );
}

#[gpui_kit::test]
async fn a_folded_read_state_command_still_updates_the_message(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let updates = Arc::new(Mutex::new(Vec::new()));
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        ReadStateProvider::new(ReadStateActionResponse::Confirm, Arc::clone(&updates)),
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    context.simulate_resize(size(px(1000.), px(WINDOW_HEIGHT)));
    open_first_cached_message(context);
    let threshold = reader_width_threshold(context, true);
    context.simulate_resize(size(threshold - px(1.), px(WINDOW_HEIGHT)));
    draw_mail_view(context);
    assert!(context.debug_bounds("message-read-state").is_none());

    let more = context
        .debug_bounds(MORE_ACTIONS)
        .expect("the folded commands sit behind the More button");
    context.simulate_click(more.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    let item = context.update(|window, _| window.find_by_label("Mark read").bounds().center());
    context.simulate_click(item, Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);

    assert_eq!(
        updates
            .lock()
            .expect("fixture read-state log mutex is healthy")
            .as_slice(),
        [(message_id, ReadState::Read)]
    );
    assert!(context.cx.update(|app| matches!(
        &view.read(app).read_state_update,
        ReadStateUpdateStatus::Confirmed {
            status: ReadState::Read,
            ..
        }
    )));
}
