//! A band that labels the group of list rows below it.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _};
use gpui_kit::{App, IntoElement, Rems, RenderOnce, SharedString, Window, div, prelude::*, rems};

/// The band's height. A list that places a header above a row adds this to that row's height.
pub(crate) const GROUP_HEADER_HEIGHT: Rems = rems(1.75);

/// A caption band for one group of rows, such as one day of messages.
///
/// The band has no accessible role. The rows it labels keep their own names and set positions.
#[derive(IntoElement)]
pub(crate) struct GroupHeader {
    id: SharedString,
    label: SharedString,
}

impl GroupHeader {
    /// Creates a band showing `label`. `id` is the element id and the test selector.
    pub(crate) fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

impl RenderOnce for GroupHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let selector = self.id.to_string();
        div()
            .id(self.id)
            .debug_selector(move || selector)
            .w_full()
            .h(GROUP_HEADER_HEIGHT)
            .flex()
            .items_center()
            .px_4()
            .bg(theme.list_head)
            .text_xs()
            .font_semibold()
            .text_color(theme.muted_foreground)
            .child(div().min_w_0().truncate().child(self.label))
    }
}
