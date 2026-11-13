//! The month a person picks a week from: which days it shows and how the keyboard moves.

use jiff::{Span, civil};

use super::super::week::monday_start;

/// The number of weeks the grid shows, so its height stays the same from month to month.
const WEEKS_SHOWN: i64 = 6;

/// The Monday-first weeks that cover one month, padded with days of the months around it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MonthGrid {
    month_start: civil::Date,
    weeks: Vec<[civil::Date; 7]>,
}

impl MonthGrid {
    /// The grid for the month that contains `date`.
    pub(super) fn containing(date: civil::Date) -> Self {
        let month_start = date.first_of_month();
        let grid_start = monday_start(month_start).unwrap_or(month_start);
        let weeks = (0..WEEKS_SHOWN)
            .map(|week| {
                [0_i64, 1, 2, 3, 4, 5, 6]
                    .map(|day| grid_start.saturating_add(Span::new().days(week * 7 + day)))
            })
            .collect();
        Self { month_start, weeks }
    }

    /// The first day of the month.
    pub(super) fn month_start(&self) -> civil::Date {
        self.month_start
    }

    pub(super) fn weeks(&self) -> &[[civil::Date; 7]] {
        &self.weeks
    }

    /// Whether the grid has a row for the week that starts on `week_start`.
    pub(super) fn shows_week(&self, week_start: civil::Date) -> bool {
        self.weeks.iter().any(|week| week[0] == week_start)
    }

    /// Whether `date` belongs to the month, not to a neighbor the grid pads with.
    pub(super) fn is_in_month(&self, date: civil::Date) -> bool {
        date.year() == self.month_start.year() && date.month() == self.month_start.month()
    }
}

/// A keyboard move of the day that has focus in the mini month.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CursorMove {
    PreviousDay,
    NextDay,
    PreviousWeek,
    NextWeek,
    PreviousMonth,
    NextMonth,
}

impl CursorMove {
    /// The day the move reaches from `cursor`. A move that leaves the supported range stays put.
    pub(super) fn apply(self, cursor: civil::Date) -> civil::Date {
        let step = match self {
            Self::PreviousDay => Span::new().days(-1),
            Self::NextDay => Span::new().days(1),
            Self::PreviousWeek => Span::new().weeks(-1),
            Self::NextWeek => Span::new().weeks(1),
            Self::PreviousMonth => Span::new().months(-1),
            Self::NextMonth => Span::new().months(1),
        };
        cursor.checked_add(step).unwrap_or(cursor)
    }
}

/// The first day of the month that the week starting on `week_start` mostly belongs to.
pub(super) fn month_of_week(week_start: civil::Date) -> civil::Date {
    week_start
        .saturating_add(Span::new().days(3))
        .first_of_month()
}

/// Whether `date` is one of the seven days that start on `week_start`.
pub(super) fn is_in_week(date: civil::Date, week_start: civil::Date) -> bool {
    date >= week_start && date <= week_start.saturating_add(Span::new().days(6))
}

#[cfg(test)]
mod tests {
    use jiff::civil;

    use super::{CursorMove, MonthGrid, is_in_week, month_of_week};

    fn date(year: i16, month: i8, day: i8) -> civil::Date {
        civil::Date::new(year, month, day).expect("synthetic date is valid")
    }

    #[test]
    fn month_grid_starts_on_the_monday_that_covers_the_first_day() {
        let grid = MonthGrid::containing(date(2030, 1, 17));

        assert_eq!(grid.month_start(), date(2030, 1, 1));
        assert_eq!(grid.weeks().len(), 6);
        assert_eq!(grid.weeks()[0][0], date(2029, 12, 31));
        assert_eq!(grid.weeks()[0][1], date(2030, 1, 1));
        assert_eq!(grid.weeks()[5][6], date(2030, 2, 10));
        assert!(grid.weeks().iter().all(|week| {
            week.windows(2)
                .all(|pair| pair[1] == pair[0].tomorrow().expect("next day is in range"))
        }));
    }

    #[test]
    fn month_grid_marks_the_padding_days_outside_the_month() {
        let grid = MonthGrid::containing(date(2030, 1, 17));

        assert!(!grid.is_in_month(date(2029, 12, 31)));
        assert!(grid.is_in_month(date(2030, 1, 1)));
        assert!(grid.is_in_month(date(2030, 1, 31)));
        assert!(!grid.is_in_month(date(2030, 2, 1)));
    }

    #[test]
    fn cursor_moves_by_day_week_and_month() {
        let cursor = date(2030, 1, 31);

        assert_eq!(CursorMove::PreviousDay.apply(cursor), date(2030, 1, 30));
        assert_eq!(CursorMove::NextDay.apply(cursor), date(2030, 2, 1));
        assert_eq!(CursorMove::PreviousWeek.apply(cursor), date(2030, 1, 24));
        assert_eq!(CursorMove::NextWeek.apply(cursor), date(2030, 2, 7));
        assert_eq!(CursorMove::NextMonth.apply(cursor), date(2030, 2, 28));
        assert_eq!(CursorMove::PreviousMonth.apply(cursor), date(2029, 12, 31));
    }

    #[test]
    fn the_month_follows_the_majority_of_a_week() {
        assert_eq!(month_of_week(date(2029, 12, 31)), date(2030, 1, 1));
        assert_eq!(month_of_week(date(2030, 1, 28)), date(2030, 1, 1));
        assert_eq!(month_of_week(date(2030, 1, 7)), date(2030, 1, 1));
    }

    #[test]
    fn a_week_holds_seven_days_from_its_monday() {
        let week_start = date(2030, 1, 7);

        assert!(is_in_week(date(2030, 1, 7), week_start));
        assert!(is_in_week(date(2030, 1, 13), week_start));
        assert!(!is_in_week(date(2030, 1, 6), week_start));
        assert!(!is_in_week(date(2030, 1, 14), week_start));
    }
}
