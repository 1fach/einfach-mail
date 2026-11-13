//! The option row of a focused listbox, with the accessibility contract of ADR 0024 and ADR 0042.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Div, ElementId, Role, Stateful, div, prelude::*};

/// Where an option sits among the options of its listbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SetPosition {
    index: usize,
    size: usize,
}

impl SetPosition {
    /// The option at zero-based `index` in a set of `size` options.
    pub(crate) fn new(index: usize, size: usize) -> Self {
        Self { index, size }
    }
}

/// An option of a listbox: a full-width, rounded row with a role, a set position, and a state.
///
/// A selected option has the active descendant mark and the list's active fill. An unselected
/// one has the list's hover fill. The caller adds the label, the content, and the click handler.
pub(crate) fn list_option(
    id: impl Into<ElementId>,
    position: SetPosition,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    div()
        .id(id)
        .w_full()
        .rounded_md()
        .text_color(theme.foreground)
        .role(Role::ListBoxOption)
        .aria_position_in_set(position.index + 1)
        .aria_size_of_set(position.size)
        .aria_selected(selected)
        .when(selected, |option| {
            option.aria_active_descendant().bg(theme.list_active)
        })
        .hover({
            let fill = if selected {
                theme.list_active
            } else {
                theme.list_hover
            };
            move |option| option.bg(fill)
        })
}
