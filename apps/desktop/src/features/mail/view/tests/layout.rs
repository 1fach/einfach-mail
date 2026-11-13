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
