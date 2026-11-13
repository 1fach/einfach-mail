//! A titled card that holds one setting or one group of related facts.

use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, Role, SharedString, Window, div,
    prelude::*, rems,
};

use super::icon::AppIcon;

/// The narrowest the title and description may get before the trailing control wraps below them.
const MIN_TEXT_WIDTH: f32 = 14.;

/// A group surface with a header row and optional body rows.
///
/// The header row shows an optional leading icon or avatar, a title, a description, and a trailing
/// control. The trailing control wraps below the text when the card is narrow. Each child added to
/// the card becomes a body row under a hairline. A card never holds another card.
#[derive(IntoElement)]
pub(crate) struct SettingsCard {
    id: &'static str,
    title: SharedString,
    leading: Option<AnyElement>,
    description: Option<AnyElement>,
    trailing: Option<AnyElement>,
    body: Vec<AnyElement>,
}

impl SettingsCard {
    /// Creates a group named by `title`. `id` is the element id and the test selector.
    pub(crate) fn new(id: &'static str, title: impl Into<SharedString>) -> Self {
        Self {
            id,
            title: title.into(),
            leading: None,
            description: None,
            trailing: None,
            body: Vec::new(),
        }
    }

    /// Puts a product icon before the title.
    pub(crate) fn icon(mut self, icon: AppIcon) -> Self {
        self.leading = Some(Icon::new(icon).size_5().into_any_element());
        self
    }

    /// Puts an avatar before the title.
    pub(crate) fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// Adds secondary text under the title. Plain text or a column of text lines both work.
    pub(crate) fn description(mut self, description: impl IntoElement) -> Self {
        self.description = Some(description.into_any_element());
        self
    }

    /// Adds the control at the end of the header row.
    pub(crate) fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl ParentElement for SettingsCard {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

impl RenderOnce for SettingsCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let id = self.id;
        let text = v_flex()
            .min_w_0()
            .gap_1()
            .child(div().text_sm().font_semibold().child(self.title.clone()))
            .children(self.description.map(|description| {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(description)
            }));
        let summary = h_flex()
            .flex_1()
            .min_w(rems(MIN_TEXT_WIDTH))
            .items_center()
            .gap_4()
            .children(
                self.leading
                    .map(|leading| div().flex_shrink_0().child(leading)),
            )
            .child(text);
        let header = h_flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_x_4()
            .gap_y_3()
            .p_4()
            .child(summary)
            .children(
                self.trailing
                    .map(|trailing| div().flex_shrink_0().child(trailing)),
            );
        v_flex()
            .id(id)
            .debug_selector(move || id.to_owned())
            .role(Role::Group)
            .aria_label(self.title)
            .w_full()
            .bg(theme.group_box)
            .text_color(theme.group_box_foreground)
            .border_1()
            .border_color(border)
            .rounded(theme.radius_lg)
            .child(header)
            .children(self.body.into_iter().map(|row| {
                div()
                    .w_full()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(border)
                    .text_sm()
                    .child(row)
            }))
    }
}
