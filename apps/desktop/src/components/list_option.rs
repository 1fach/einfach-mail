//! The option row of a focused listbox, with the accessibility contract of ADR 0024 and ADR 0042.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Div, ElementId, Hsla, Role, Stateful, div, prelude::*};

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

/// The fills of an option that is not selected, when it does not use the list's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OptionFill {
    /// The fill at rest.
    pub(crate) rest: Hsla,
    /// The fill under the pointer.
    pub(crate) hover: Hsla,
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
    option(id, position, selected, None, cx)
}

/// An option like [`list_option`] whose unselected fills are `fill`, for options that carry a
/// tint of their own, such as the bars of a calendar.
pub(crate) fn filled_list_option(
    id: impl Into<ElementId>,
    position: SetPosition,
    selected: bool,
    fill: OptionFill,
    cx: &App,
) -> Stateful<Div> {
    option(id, position, selected, Some(fill), cx)
}

fn option(
    id: impl Into<ElementId>,
    position: SetPosition,
    selected: bool,
    fill: Option<OptionFill>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let hover_fill = if selected {
        theme.list_active
    } else {
        fill.map_or(theme.list_hover, |fill| fill.hover)
    };
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
        .when_some(fill.filter(|_| !selected), |option, fill| {
            option.bg(fill.rest)
        })
        .hover(move |option| option.bg(hover_fill))
}
