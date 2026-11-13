//! Dimensions the application shell shares with the features it hosts.

use gpui_kit::{Rems, rems};

/// The width of the navigation pane. Mail reads it to decide when the window is too narrow for
/// the message list and reader side by side.
pub(crate) const NAVIGATION_PANE_WIDTH: Rems = rems(16.);
