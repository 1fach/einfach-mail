//! The local calendar a person reads dates in: the system time zone and today's date.

use std::time::SystemTime;

use jiff::{Timestamp, civil, tz::TimeZone};

/// A time zone and the date that counts as today in it.
///
/// A system clock reads the current date on every call, so labels such as "Today" stay right
/// in a window that stays open past midnight.
#[derive(Clone, Debug)]
pub(crate) struct LocalClock {
    zone: TimeZone,
    pinned_today: Option<civil::Date>,
}

impl LocalClock {
    /// The system time zone, or UTC when the system names no IANA zone.
    pub(crate) fn system() -> Self {
        Self {
            zone: Self::system_time_zone().unwrap_or(TimeZone::UTC),
            pinned_today: None,
        }
    }

    /// A clock that always reports `today`, so tests do not depend on the current date.
    #[cfg(test)]
    pub(crate) fn fixed(zone: TimeZone, today: civil::Date) -> Self {
        Self {
            zone,
            pinned_today: Some(today),
        }
    }

    /// The system time zone when it has an IANA name.
    pub(crate) fn system_time_zone() -> Option<TimeZone> {
        TimeZone::try_system()
            .ok()
            .filter(|time_zone| time_zone.iana_name().is_some())
    }

    /// The current date in `zone`.
    pub(crate) fn today_in(zone: &TimeZone) -> Option<civil::Date> {
        let now = Timestamp::try_from(SystemTime::now()).ok()?;
        Some(now.to_zoned(zone.clone()).date())
    }

    /// The date that counts as today.
    pub(crate) fn today(&self) -> civil::Date {
        self.pinned_today
            .or_else(|| Self::today_in(&self.zone))
            .unwrap_or_else(|| civil::date(1970, 1, 1))
    }

    /// The wall-clock date and time of `instant` in this zone, or `None` when it is out of range.
    pub(crate) fn local_datetime(&self, instant: SystemTime) -> Option<civil::DateTime> {
        let timestamp = Timestamp::try_from(instant).ok()?;
        Some(timestamp.to_zoned(self.zone.clone()).datetime())
    }
}

#[cfg(test)]
mod tests;
