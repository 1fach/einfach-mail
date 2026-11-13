use super::*;
use crate::components::NAVIGATION_PANE_WIDTH;

use super::super::{
    MAIL_SPLIT_HANDLE_RESERVE, MESSAGE_LIST_DEFAULT_WIDTH, MESSAGE_READER_MIN_WIDTH,
};

#[gpui_kit::test]
async fn mail_goes_compact_below_the_navigation_pane_list_and_reader_minimums(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let (profile, cache, _) = fixture(synthetic_account_id());
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
    wait_for_mail_sync_to_finish(context, &view).await;
    let rem_size = context.update(|window, _| window.rem_size());
    let wide_minimum = [
        NAVIGATION_PANE_WIDTH,
        MESSAGE_LIST_DEFAULT_WIDTH,
        MESSAGE_READER_MIN_WIDTH,
        MAIL_SPLIT_HANDLE_RESERVE,
    ]
    .into_iter()
    .map(|width| width.to_pixels(rem_size))
    .fold(px(0.), |total, width| total + width);

    let layout_at = |context: &mut VisualTestContext, width: gpui_kit::Pixels| {
        context.simulate_resize(size(width, px(800.)));
        context.run_until_parked();
        draw_mail_view(context);
        context.update(|window, _| window.try_find_by_label("Message reader").is_some())
    };

    assert!(
        layout_at(context, wide_minimum),
        "the list and reader sit side by side at the minimum width"
    );
    assert!(
        !layout_at(context, wide_minimum - px(1.)),
        "one pixel below the minimum shows a single pane"
    );
}

#[gpui_kit::test]
async fn empty_reader_state_is_centered_in_the_split_reader(cx: &mut TestAppContext) {
    assert_empty_reader_state_is_centered(cx, px(1280.), false).await;
}

#[gpui_kit::test]
async fn empty_reader_state_is_centered_in_the_compact_reader(cx: &mut TestAppContext) {
    assert_empty_reader_state_is_centered(cx, px(900.), true).await;
}

async fn assert_empty_reader_state_is_centered(
    cx: &mut TestAppContext,
    width: gpui_kit::Pixels,
    compact: bool,
) {
    cx.update(gpui_kit::init);
    let (profile, cache, _) = fixture(synthetic_account_id());
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
    context.simulate_resize(size(width, px(800.)));
    wait_for_mail_sync_to_finish(context, &view).await;
    view.update(context, |view, cx| {
        // The offline fixture never reconciles, so clear the notice a reconciled sync clears.
        view.move_recovery_reason = None;
        if compact {
            view.compact_pane = CompactMailPane::MessageReader;
        }
        cx.notify();
    });
    context.run_until_parked();
    draw_mail_view(context);
    let reader = context.update(|window, _| window.find("mail-reader").bounds());
    let empty_state = context.update(|window, _| window.find("mail-reader-empty-state").bounds());
    let tolerance = px(1.);
    assert!(
        (empty_state.center().x - reader.center().x).abs() <= tolerance,
        "empty state {empty_state:?} is centered horizontally in the reader {reader:?}"
    );
    assert!(
        (empty_state.center().y - reader.center().y).abs() <= tolerance,
        "empty state {empty_state:?} is centered vertically in the reader {reader:?}"
    );
}
