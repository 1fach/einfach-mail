//! The header above a workspace pane: its title, a tag, a status caption, and icon actions.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, IntoElement, Rems, RenderOnce, Role, SharedString, Window, div, prelude::*,
    rems,
};

const HEADER_HEIGHT: Rems = rems(3.5);

/// A pane header that reads left to right as a title, an optional tag, a status caption that
/// takes the remaining width, and a row of actions.
#[derive(IntoElement)]
pub(crate) struct PaneHeader {
    id: SharedString,
    title: SharedString,
    tag: Option<HeaderTag>,
    status: Option<AnyElement>,
    actions: Vec<AnyElement>,
}

/// The text of a tag and, when assistive technology should announce it, the status that names it.
struct HeaderTag {
    text: SharedString,
    status: Option<TagStatus>,
}

struct TagStatus {
    id: &'static str,
    accessible_label: SharedString,
}

impl PaneHeader {
    /// Creates a header named `title`. `id` is the test selector of the title.
    pub(crate) fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            tag: None,
            status: None,
            actions: Vec::new(),
        }
    }

    /// Adds a short outlined tag after the title, such as "Read-only".
    pub(crate) fn tag(mut self, tag: impl Into<SharedString>) -> Self {
        self.tag = Some(HeaderTag {
            text: tag.into(),
            status: None,
        });
        self
    }

    /// Adds a tag that is also a status, so assistive technology reads `accessible_label`.
    /// `id` is the element id and the test selector of the tag.
    pub(crate) fn tag_status(
        mut self,
        id: &'static str,
        accessible_label: impl Into<SharedString>,
        tag: impl Into<SharedString>,
    ) -> Self {
        self.tag = Some(HeaderTag {
            text: tag.into(),
            status: Some(TagStatus {
                id,
                accessible_label: accessible_label.into(),
            }),
        });
        self
    }

    /// Adds the status caption. The caption owns its truncation and its accessible role.
    pub(crate) fn status(mut self, status: impl IntoElement) -> Self {
        self.status = Some(status.into_any_element());
        self
    }

    /// Adds an action at the end of the header.
    pub(crate) fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl RenderOnce for PaneHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let title_selector = self.id.to_string();
        let title = div()
            .id(self.id)
            .debug_selector(move || title_selector)
            .min_w_0()
            .truncate()
            .text_xl()
            .font_semibold()
            .child(self.title);
        let tag = self.tag.map(|tag| {
            let element = div()
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(tag.text);
            match tag.status {
                Some(status) => {
                    let selector = status.id.to_owned();
                    let element = element
                        .id(status.id)
                        .debug_selector(move || selector)
                        .role(Role::Status)
                        .aria_label(status.accessible_label);
                    #[cfg(test)]
                    let element = element.test_support();
                    element.into_any_element()
                }
                None => element.into_any_element(),
            }
        });
        h_flex()
            .w_full()
            .h(HEADER_HEIGHT)
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .px_4()
            .border_b_1()
            .border_color(theme.border)
            .child(title)
            .children(tag)
            .child(div().flex_1().min_w_0().children(self.status))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .children(self.actions),
            )
    }
}
