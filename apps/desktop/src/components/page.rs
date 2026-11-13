//! A top-aligned settings-style page: a scrolling viewport around one centered column.

use gpui_kit::component::{
    ActiveTheme as _, StyledExt as _, scroll::ScrollableElement as _, v_flex,
};
use gpui_kit::{
    AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Window, div,
    prelude::*, rems,
};

/// The page column: 40rem of content plus the 2rem page padding on each side.
const PAGE_COLUMN_WIDTH: f32 = 44.;

/// A titled page whose children stack in one centered column.
///
/// The page owns the scroll viewport, so its id names the element that scrolls.
#[derive(IntoElement)]
pub(crate) struct Page {
    scroll_id: ElementId,
    title: SharedString,
    description: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl Page {
    pub(crate) fn new(scroll_id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            scroll_id: scroll_id.into(),
            title: title.into(),
            description: None,
            children: Vec::new(),
        }
    }

    /// Adds a line under the title that says what the page is for.
    pub(crate) fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl ParentElement for Page {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Page {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let heading = v_flex()
            .gap_1()
            .child(div().text_2xl().font_semibold().child(self.title))
            .children(self.description.map(|description| {
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(description)
            }));
        div()
            .id(self.scroll_id)
            .size_full()
            .overflow_y_scrollbar()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                v_flex()
                    .w_full()
                    .max_w(rems(PAGE_COLUMN_WIDTH))
                    .mx_auto()
                    .p_8()
                    .gap_6()
                    .child(heading)
                    .children(self.children),
            )
    }
}
