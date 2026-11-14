use einfach_mail_domain::{MessageMetadataPatch, MessagePreview};
use jiff::{civil, tz::TimeZone};

use super::*;
use crate::components::LocalClock;
use crate::features::mail::view::message_list::MessageRowSummary;

const SHORT_PREVIEW: &str = "Thanks for sending the notes.";

fn preview(text: &str) -> MessagePreview {
    MessagePreview::from_untrusted(text).expect("the synthetic preview has printable text")
}

fn long_preview() -> MessagePreview {
    preview(&"Agenda item with a deliberately long preview. ".repeat(12))
}

/// Opens a list of three messages: a long preview, no preview, and a short preview.
async fn open_previewed_list(cx: &mut TestAppContext) -> (&mut VisualTestContext, Vec<MessageId>) {
    cx.update(gpui_kit::init);
    let (profile, cache, _) = fixture(synthetic_account_id());
    let ids = append_synthetic_messages(&cache, 3);
    let previews = [Some(long_preview()), None, Some(preview(SHORT_PREVIEW))];
    for (message, preview) in cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .messages
        .iter_mut()
        .zip(previews)
    {
        let mut patch = MessageMetadataPatch::new();
        patch.set_preview(preview);
        message.apply_patch(patch);
    }
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
    (context, ids)
}

fn bounds_of(
    context: &mut VisualTestContext,
    selector: &str,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    context
        .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .unwrap_or_else(|| panic!("{selector} is rendered"))
}

#[gpui_kit::test]
async fn rows_keep_one_height_with_and_without_a_preview(cx: &mut TestAppContext) {
    let (context, ids) = open_previewed_list(cx).await;

    for id in &ids {
        let row = bounds_of(context, &format!("message-row-{}", id.as_str()));
        assert_eq!(row.size.height, px(88.), "{} is 5.5rem tall", id.as_str());
    }
}

#[gpui_kit::test]
async fn the_preview_line_is_reserved_and_truncated_to_one_line(cx: &mut TestAppContext) {
    let (context, ids) = open_previewed_list(cx).await;

    let mut offsets = Vec::new();
    for id in &ids {
        let row = bounds_of(context, &format!("message-row-{}", id.as_str()));
        let line = bounds_of(context, &format!("message-preview-{}", id.as_str()));
        assert_eq!(
            line.size.height,
            px(16.),
            "{} has a one-line slot",
            id.as_str()
        );
        assert!(
            line.right() <= row.right() && line.bottom() <= row.bottom(),
            "{line:?} stays inside {row:?}"
        );
        offsets.push((line.top() - row.top(), line.left() - row.left()));
    }
    assert!(
        offsets.windows(2).all(|pair| pair[0] == pair[1]),
        "rows with and without a preview place the line alike: {offsets:?}"
    );
}

#[gpui_kit::test]
async fn the_preview_joins_the_accessible_details_and_leaves_the_name_alone(
    cx: &mut TestAppContext,
) {
    let (context, _) = open_previewed_list(cx).await;

    let options =
        context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBoxOption));
    let labels: Vec<_> = options.iter().map(|option| option.label()).collect();
    let descriptions: Vec<_> = options
        .iter()
        .map(|option| option.description().unwrap_or_default())
        .collect();

    assert_eq!(
        labels,
        [
            Some(SYNTHETIC_MESSAGE_LABEL_PREFIX),
            Some("Ada Example <ada@example.test>. Synthetic subject 2"),
            Some("Ada Example <ada@example.test>. Synthetic subject 3"),
        ]
    );
    assert!(descriptions[0].ends_with(long_preview().as_str()));
    assert!(!descriptions[1].contains("Agenda") && !descriptions[1].contains(SHORT_PREVIEW));
    assert!(descriptions[2].ends_with(SHORT_PREVIEW));
}

#[test]
fn the_row_summary_appends_the_preview_after_the_flag() {
    let clock = LocalClock::fixed(TimeZone::UTC, civil::date(2027, 1, 15));
    let (_, cache, _) = fixture(synthetic_account_id());
    let mut message = cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .messages[0]
        .clone();
    assert_eq!(
        MessageRowSummary::new(&message, &clock).accessible_details(),
        "Unread. Today, 08:00"
    );

    let mut patch = MessageMetadataPatch::new();
    patch.set_preview(Some(preview(SHORT_PREVIEW)));
    patch.set_follow_up_flag_status(Some(einfach_mail_domain::FollowUpFlagStatus::Flagged));
    message.apply_patch(patch);
    let summary = MessageRowSummary::new(&message, &clock);

    assert_eq!(
        summary.accessible_details(),
        "Unread. Today, 08:00. Flagged. Thanks for sending the notes."
    );
    assert_eq!(
        summary.accessible_label(),
        "Ada Example <ada@example.test>. Synthetic subject"
    );
}
