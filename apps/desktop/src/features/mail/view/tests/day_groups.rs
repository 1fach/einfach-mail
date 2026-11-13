use std::time::SystemTime;

use einfach_mail_domain::{FollowUpFlagStatus, MessageMetadataPatch};
use gpui_kit::{Bounds, Pixels};
use jiff::{civil, tz::TimeZone};

use super::*;
use crate::components::{GROUP_HEADER_HEIGHT, LocalClock};
use crate::features::mail::view::message_list::MessageRowSummary;

fn today() -> civil::Date {
    civil::date(2026, 11, 13)
}

fn test_clock() -> LocalClock {
    LocalClock::fixed(TimeZone::UTC, today())
}

#[derive(Clone, Copy)]
struct DatedMessage {
    received: civil::DateTime,
    unread: bool,
    flagged: bool,
}

/// Newest first: two messages today, two yesterday, and one in October.
fn dated_messages() -> [DatedMessage; 5] {
    let at = |date: civil::Date, hour, minute| date.at(hour, minute, 0, 0);
    let message = |received, unread, flagged| DatedMessage {
        received,
        unread,
        flagged,
    };
    [
        message(at(today(), 10, 0), true, false),
        message(at(today(), 8, 30), false, true),
        message(at(civil::date(2026, 11, 12), 20, 0), false, false),
        message(at(civil::date(2026, 11, 12), 9, 0), true, true),
        message(at(civil::date(2026, 10, 25), 9, 0), false, false),
    ]
}

const STARTS_GROUP: [bool; 5] = [true, false, true, false, true];

async fn open_dated_message_list<'a>(
    cx: &'a mut TestAppContext,
    dated: &[DatedMessage],
    window_size: gpui_kit::Size<Pixels>,
) -> (&'a mut VisualTestContext, Entity<MailView>, Vec<MessageId>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_message_navigation_keys(cx);
    });
    let (profile, cache, _) = fixture(synthetic_account_id());
    let ids = append_synthetic_messages(&cache, dated.len());
    for (message, dated) in cache
        .0
        .lock()
        .expect("fixture cache mutex is healthy")
        .messages
        .iter_mut()
        .zip(dated)
    {
        let received: SystemTime = dated
            .received
            .to_zoned(TimeZone::UTC)
            .expect("the synthetic instant is in range")
            .timestamp()
            .into();
        let mut patch = MessageMetadataPatch::new();
        patch.set_received_at(received);
        patch.set_read_state(if dated.unread {
            ReadState::Unread
        } else {
            ReadState::Read
        });
        patch.set_follow_up_flag_status(dated.flagged.then_some(FollowUpFlagStatus::Flagged));
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
        let view = cx.new(|cx| {
            let mut view = test_mail_view(profile, workflow, cx);
            view.clock = test_clock();
            view
        });
        mail_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = mail_view.expect("mail view is created with the test window");
    context.simulate_resize(window_size);
    wait_for_mail_sync_to_finish(context, &view).await;
    draw_mail_view(context);
    (context, view, ids)
}

/// Looks up a debug selector built at run time. `debug_bounds` takes a `'static` selector, so the
/// few short strings a test builds are leaked.
fn bounds_of(context: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    context.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn selected_message(context: &mut VisualTestContext, view: &Entity<MailView>) -> Option<MessageId> {
    context
        .cx
        .update(|cx| view.read(cx).selected_message_id().cloned())
}

async fn press_and_settle(context: &mut VisualTestContext, key: &str) {
    press_window_key(context, key);
    context.run_until_parked();
    draw_mail_view(context);
}

fn focus_message_list(context: &mut VisualTestContext, view: &Entity<MailView>) {
    context.update(gpui_kit::Window::focus_next);
    for _ in 0..20 {
        press_window_key(context, "tab");
        if context.update(|window, cx| view.read(cx).message_list_focus.is_focused(window)) {
            return;
        }
    }
    panic!("Tab reaches the cached message list");
}

#[gpui_kit::test]
async fn a_day_header_sits_above_only_the_first_message_of_each_day(cx: &mut TestAppContext) {
    let (context, _, ids) =
        open_dated_message_list(cx, &dated_messages(), size(px(1600.), px(900.))).await;
    let header_height = GROUP_HEADER_HEIGHT.to_pixels(px(16.));

    for (id, starts_group) in ids.iter().zip(STARTS_GROUP) {
        let header = bounds_of(context, &format!("message-day-header-{}", id.as_str()));
        let row = bounds_of(context, &format!("message-row-{}", id.as_str()))
            .expect("every message row is rendered");
        assert_eq!(
            header.is_some(),
            starts_group,
            "{} shows a day header only when it starts a group",
            id.as_str()
        );
        if let Some(header) = header {
            assert_eq!(header.size.height, header_height);
            assert!(
                (header.bottom() - row.top()).abs() < px(0.5),
                "the header {header:?} ends where its row {row:?} begins"
            );
        }
    }
}

#[gpui_kit::test]
async fn day_headers_are_not_options_and_leave_the_option_descriptions_in_message_order(
    cx: &mut TestAppContext,
) {
    let (context, _, ids) =
        open_dated_message_list(cx, &dated_messages(), size(px(1600.), px(900.))).await;

    let options =
        context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBoxOption));
    let descriptions: Vec<_> = options.iter().map(|option| option.description()).collect();

    assert_eq!(
        options.len(),
        ids.len(),
        "only messages are options, so the set size excludes the day headers"
    );
    assert_eq!(
        descriptions,
        [
            Some("Unread. Today, 10:00"),
            Some("Today, 08:30. Flagged"),
            Some("Yesterday, 20:00"),
            Some("Unread. Yesterday, 09:00. Flagged"),
            Some("Sunday, October 25, 2026, 09:00"),
        ]
    );
}

#[gpui_kit::test]
async fn unread_rows_are_marked_by_weight_and_a_dot_and_read_rows_by_neither(
    cx: &mut TestAppContext,
) {
    let dated = dated_messages();
    let (context, view, ids) = open_dated_message_list(cx, &dated, size(px(1600.), px(900.))).await;

    for (index, (id, dated)) in ids.iter().zip(dated).enumerate() {
        let dot = bounds_of(context, &format!("message-unread-dot-{}", id.as_str()));
        let weight = context.cx.update(|cx| {
            let view = view.read(cx);
            MessageRowSummary::new(&view.messages[index], &view.clock).text_weight()
        });
        assert_eq!(dot.is_some(), dated.unread, "dot for {}", id.as_str());
        assert_eq!(
            weight == gpui_kit::FontWeight::SEMIBOLD,
            dated.unread,
            "weight for {}",
            id.as_str()
        );
        assert!(
            dated.unread || weight == gpui_kit::FontWeight::NORMAL,
            "a read row is regular weight"
        );
    }
}

#[gpui_kit::test]
async fn row_content_shares_one_leading_and_one_trailing_edge_across_rows(cx: &mut TestAppContext) {
    let (context, _, ids) =
        open_dated_message_list(cx, &dated_messages(), size(px(1600.), px(900.))).await;
    let id = |index: usize| ids[index].as_str();

    let avatars: Vec<_> = (0..ids.len())
        .map(|index| {
            bounds_of(context, &format!("message-avatar-{}", id(index)))
                .expect("every row shows an avatar")
        })
        .collect();
    let times: Vec<_> = (0..ids.len())
        .map(|index| {
            bounds_of(context, &format!("message-time-{}", id(index)))
                .expect("every row shows a time")
        })
        .collect();

    assert!(
        avatars
            .iter()
            .all(|avatar| (avatar.left() - avatars[0].left()).abs() < px(0.5)),
        "avatars share one x: {avatars:?}"
    );
    assert!(
        times
            .iter()
            .all(|time| (time.right() - times[0].right()).abs() < px(0.5)),
        "times share one trailing edge: {times:?}"
    );
    for (index, dated) in dated_messages().iter().enumerate() {
        let flag = bounds_of(context, &format!("message-flag-{}", id(index)));
        assert_eq!(flag.is_some(), dated.flagged);
        if let Some(flag) = flag {
            assert!(
                (flag.right() - times[0].right()).abs() < px(0.5),
                "the flag {flag:?} sits in the time's trailing lane {:?}",
                times[0]
            );
        }
    }
}

#[gpui_kit::test]
async fn arrow_home_and_end_keys_move_across_day_boundaries_and_keep_the_row_in_view(
    cx: &mut TestAppContext,
) {
    let (context, view, ids) =
        open_dated_message_list(cx, &dated_messages(), size(px(1600.), px(420.))).await;
    focus_message_list(context, &view);

    let mut visited = Vec::new();
    for _ in 0..ids.len() {
        press_and_settle(context, "down").await;
        visited.push(selected_message(context, &view));
    }
    let expected: Vec<_> = ids.iter().cloned().map(Some).collect();
    assert_eq!(visited, expected, "Down walks every message in order");

    press_and_settle(context, "down").await;
    assert_eq!(selected_message(context, &view), ids.last().cloned());
    let list = context.update(|window, _| window.find("cached-message-list").bounds());
    let last_row = bounds_of(context, &format!("message-row-{}", ids[4].as_str()))
        .expect("the last row is rendered after scrolling to it");
    assert!(
        last_row.bottom() <= list.bottom(),
        "the last row {last_row:?} is inside the list {list:?}"
    );

    press_and_settle(context, "up").await;
    assert_eq!(selected_message(context, &view), Some(ids[3].clone()));
    press_and_settle(context, "up").await;
    assert_eq!(
        selected_message(context, &view),
        Some(ids[2].clone()),
        "Up crosses from the first message of yesterday to the last of today"
    );

    press_and_settle(context, "home").await;
    assert_eq!(selected_message(context, &view), ids.first().cloned());
    press_and_settle(context, "end").await;
    assert_eq!(selected_message(context, &view), ids.last().cloned());
}

#[gpui_kit::test]
async fn the_pane_header_names_the_selected_folder_and_keeps_its_actions(cx: &mut TestAppContext) {
    let (context, view, _) =
        open_dated_message_list(cx, &dated_messages(), size(px(1600.), px(900.))).await;

    assert_eq!(
        context
            .cx
            .update(|cx| view.read(cx).selected_folder_name().to_owned()),
        "Inbox"
    );
    assert!(bounds_of(context, "mail-folder-title").is_some());
    for label in ["Sync", "Narrow list", "Widen list"] {
        assert_eq!(
            context.update(|window, _| window.find_by_label(label).role()),
            Some(gpui_kit::Role::Button),
            "{label} is a named button"
        );
    }
}
