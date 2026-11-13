//! Groups the message list by the local day each message was received.

use std::rc::Rc;

use einfach_mail_domain::MailMessage;
use gpui_kit::{Pixels, Size, size};
use jiff::civil;

use super::MESSAGE_ROW_HEIGHT;
use crate::components::{GROUP_HEADER_HEIGHT, LocalClock};

/// The local day of one message and whether a day header sits above it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MessageDay {
    date: civil::Date,
    starts_group: bool,
}

impl MessageDay {
    pub(super) fn date(self) -> civil::Date {
        self.date
    }

    pub(super) fn starts_group(self) -> bool {
        self.starts_group
    }
}

/// The local date and time `message` was received, or the earliest representable moment when the
/// received time is out of range.
pub(super) fn received_local(message: &MailMessage, clock: &LocalClock) -> civil::DateTime {
    clock
        .local_datetime(*message.metadata().received_at())
        .unwrap_or(civil::DateTime::MIN)
}

/// Marks each message that begins a new local day.
///
/// A group starts when the date differs from the previous message's, so input that is not sorted
/// still gets a header wherever the day changes.
pub(super) fn message_days(messages: &[MailMessage], clock: &LocalClock) -> Rc<[MessageDay]> {
    let mut previous: Option<civil::Date> = None;
    messages
        .iter()
        .map(|message| {
            let date = received_local(message, clock).date();
            let starts_group = previous != Some(date);
            previous = Some(date);
            MessageDay { date, starts_group }
        })
        .collect()
}

/// The virtual list's item sizes: a row, plus a header height for each message that starts a group.
pub(super) fn row_sizes(days: &[MessageDay], rem_size: Pixels) -> Rc<Vec<Size<Pixels>>> {
    let row_height = MESSAGE_ROW_HEIGHT.to_pixels(rem_size);
    let header_height = GROUP_HEADER_HEIGHT.to_pixels(rem_size);
    Rc::new(
        days.iter()
            .map(|day| {
                let height = if day.starts_group {
                    header_height + row_height
                } else {
                    row_height
                };
                size(Pixels::ZERO, height)
            })
            .collect(),
    )
}

/// The caption of a day group: "Today", "Yesterday", or a full date such as
/// "Sunday, October 25, 2026".
pub(super) fn day_label(date: civil::Date, today: civil::Date) -> String {
    if date == today {
        "Today".to_owned()
    } else if date.tomorrow().is_ok_and(|next| next == today) {
        "Yesterday".to_owned()
    } else {
        date.strftime("%A, %B %-d, %Y").to_string()
    }
}

/// The time shown on a message row. The day group header above the row carries the date.
pub(super) fn time_label(received: civil::DateTime) -> String {
    received.strftime("%H:%M").to_string()
}

#[cfg(test)]
mod tests;
