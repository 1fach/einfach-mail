use std::time::SystemTime;

use einfach_mail_domain::{
    FolderId, FolderReference, MailMessage, Mailbox, MessageId, MessageMetadata, MessageOrigin,
    ReadState,
};
use gpui_kit::px;
use jiff::{civil, tz::TimeZone};

use super::{MessageDay, day_label, message_days, row_sizes, time_label};
use crate::components::LocalClock;
use crate::test_support::synthetic_account_id;

fn today() -> civil::Date {
    civil::date(2026, 11, 13)
}

fn message_received_at(id: &str, received: SystemTime) -> MailMessage {
    let folder = FolderReference::new(
        synthetic_account_id(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    let sender = Mailbox::try_new(
        "ada@example.test".to_owned(),
        Some("Ada Example".to_owned()),
    )
    .expect("synthetic mailbox is valid");
    let metadata = MessageMetadata::new(
        Some("Synthetic subject".to_owned()),
        MessageOrigin::new(Some(sender), None),
        received,
        ReadState::Read,
        None,
    );
    MailMessage::new(
        folder,
        MessageId::try_from(id.to_owned()).expect("synthetic message ID is valid"),
        metadata,
    )
}

fn utc_instant(datetime: civil::DateTime) -> SystemTime {
    datetime
        .to_zoned(TimeZone::UTC)
        .expect("the synthetic instant is in range")
        .timestamp()
        .into()
}

fn starts_groups(days: &[MessageDay]) -> Vec<bool> {
    days.iter().map(|day| day.starts_group()).collect()
}

#[test]
fn day_labels_name_today_and_yesterday_and_spell_out_older_dates() {
    assert_eq!(day_label(today(), today()), "Today");
    assert_eq!(day_label(civil::date(2026, 11, 12), today()), "Yesterday");
    assert_eq!(
        day_label(civil::date(2026, 10, 25), today()),
        "Sunday, October 25, 2026"
    );
    assert_eq!(
        day_label(civil::date(2025, 1, 3), today()),
        "Friday, January 3, 2025"
    );
}

#[test]
fn yesterday_follows_the_calendar_across_a_month_boundary() {
    let first_of_month = civil::date(2026, 12, 1);

    assert_eq!(
        day_label(civil::date(2026, 11, 30), first_of_month),
        "Yesterday"
    );
}

#[test]
fn time_labels_show_the_24_hour_time_for_every_day() {
    let cases = [
        (today().at(9, 14, 0, 0), "09:14", "today"),
        (
            civil::date(2026, 11, 12).at(18, 5, 0, 0),
            "18:05",
            "yesterday",
        ),
        (
            civil::date(2025, 12, 31).at(0, 0, 0, 0),
            "00:00",
            "another year",
        ),
    ];

    for (received, expected, reason) in cases {
        assert_eq!(time_label(received), expected, "{reason}");
    }
}

#[test]
fn a_group_starts_where_the_local_date_changes() {
    let clock = LocalClock::fixed(TimeZone::UTC, today());
    let messages = [
        message_received_at("m1", utc_instant(civil::date(2026, 11, 13).at(10, 0, 0, 0))),
        message_received_at("m2", utc_instant(civil::date(2026, 11, 13).at(8, 0, 0, 0))),
        message_received_at("m3", utc_instant(civil::date(2026, 11, 12).at(23, 0, 0, 0))),
        message_received_at("m4", utc_instant(civil::date(2026, 10, 25).at(9, 0, 0, 0))),
    ];

    let days = message_days(&messages, &clock);

    assert_eq!(starts_groups(&days), [true, false, true, true]);
    assert_eq!(days[2].date(), civil::date(2026, 11, 12));
}

#[test]
fn groups_use_the_local_day_not_the_utc_day() {
    let new_york = TimeZone::get("America/New_York")
        .expect("the synthetic IANA zone exists in Linux zoneinfo");
    let clock = LocalClock::fixed(new_york, today());
    let messages = [
        message_received_at("m1", utc_instant(civil::date(2026, 11, 13).at(4, 0, 0, 0))),
        message_received_at("m2", utc_instant(civil::date(2026, 11, 13).at(2, 0, 0, 0))),
    ];

    let days = message_days(&messages, &clock);

    assert_eq!(
        starts_groups(&days),
        [true, false],
        "both messages arrived on the evening of the 12th in New York"
    );
    assert_eq!(days[0].date(), civil::date(2026, 11, 12));
}

#[test]
fn unsorted_messages_get_a_header_wherever_the_day_changes() {
    let clock = LocalClock::fixed(TimeZone::UTC, today());
    let messages = [
        message_received_at("m1", utc_instant(civil::date(2026, 11, 13).at(10, 0, 0, 0))),
        message_received_at("m2", utc_instant(civil::date(2026, 11, 12).at(10, 0, 0, 0))),
        message_received_at("m3", utc_instant(civil::date(2026, 11, 13).at(9, 0, 0, 0))),
    ];

    let days = message_days(&messages, &clock);

    assert_eq!(starts_groups(&days), [true, true, true]);
}

#[test]
fn an_empty_list_has_no_groups() {
    let clock = LocalClock::fixed(TimeZone::UTC, today());

    assert!(message_days(&[], &clock).is_empty());
}

#[test]
fn only_a_group_start_adds_the_header_height_to_its_row() {
    let days = [
        MessageDay {
            date: civil::date(2026, 11, 13),
            starts_group: true,
        },
        MessageDay {
            date: civil::date(2026, 11, 13),
            starts_group: false,
        },
        MessageDay {
            date: civil::date(2026, 11, 12),
            starts_group: true,
        },
    ];

    let heights: Vec<_> = row_sizes(&days, px(16.))
        .iter()
        .map(|size| size.height)
        .collect();

    assert_eq!(heights, [px(100.), px(72.), px(100.)]);
}
