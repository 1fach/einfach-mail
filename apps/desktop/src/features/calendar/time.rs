use jiff::{Timestamp, civil};
use time::{Date, Month, OffsetDateTime};

pub(super) fn to_calendar_date(date: civil::Date) -> Option<Date> {
    let month = Month::try_from(u8::try_from(date.month()).ok()?).ok()?;
    Date::from_calendar_date(
        i32::from(date.year()),
        month,
        u8::try_from(date.day()).ok()?,
    )
    .ok()
}

pub(super) fn to_offset_date_time(timestamp: Timestamp) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(timestamp.as_nanosecond()).ok()
}

pub(super) fn to_timestamp(date_time: OffsetDateTime) -> Option<Timestamp> {
    Timestamp::new(
        date_time.unix_timestamp(),
        i32::try_from(date_time.nanosecond()).ok()?,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::{to_offset_date_time, to_timestamp};

    #[test]
    fn timestamp_conversion_round_trips_fractional_instants_across_unix_epoch() {
        for timestamp in [
            Timestamp::new(-1, 999_999_999).expect("synthetic timestamp fits the Jiff range"),
            Timestamp::new(0, 1).expect("synthetic timestamp fits the Jiff range"),
        ] {
            let date_time =
                to_offset_date_time(timestamp).expect("synthetic timestamp fits the time range");

            assert_eq!(date_time.unix_timestamp_nanos(), timestamp.as_nanosecond());
            assert_eq!(to_timestamp(date_time), Some(timestamp));
        }
    }
}
