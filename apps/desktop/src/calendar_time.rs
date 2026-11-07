use jiff::Timestamp;
use time::OffsetDateTime;

pub(super) fn to_offset_date_time(timestamp: Timestamp) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(timestamp.as_second())
        .ok()?
        .replace_nanosecond(u32::try_from(timestamp.subsec_nanosecond()).ok()?)
        .ok()
}
