#[derive(Clone, Copy)]
pub(super) enum ListNavigation {
    Previous,
    Next,
    First,
    Last,
}

impl ListNavigation {
    pub(super) fn target_index(
        self,
        selected_index: Option<usize>,
        item_count: usize,
    ) -> Option<usize> {
        let last_index = item_count.checked_sub(1)?;
        let selected_index = selected_index.map(|index| index.min(last_index));

        Some(match self {
            Self::Previous => selected_index.map_or(last_index, |index| index.saturating_sub(1)),
            Self::Next => selected_index.map_or(0, |index| (index + 1).min(last_index)),
            Self::First => 0,
            Self::Last => last_index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ListNavigation;

    #[test]
    fn list_navigation_handles_empty_lists_and_stops_at_boundaries() {
        let cases = [
            (ListNavigation::Previous, None, 0, None),
            (ListNavigation::Next, None, 0, None),
            (ListNavigation::First, None, 0, None),
            (ListNavigation::Last, None, 0, None),
            (ListNavigation::Previous, None, 3, Some(2)),
            (ListNavigation::Next, None, 3, Some(0)),
            (ListNavigation::Previous, Some(0), 3, Some(0)),
            (ListNavigation::Next, Some(2), 3, Some(2)),
            (ListNavigation::Previous, Some(usize::MAX), 3, Some(1)),
            (ListNavigation::Next, Some(usize::MAX), 3, Some(2)),
            (ListNavigation::First, Some(1), 3, Some(0)),
            (ListNavigation::Last, Some(1), 3, Some(2)),
        ];

        for (navigation, selected_index, item_count, expected_index) in cases {
            assert_eq!(
                navigation.target_index(selected_index, item_count),
                expected_index
            );
        }
    }
}
