//! The navigation item: one destination or folder in the navigation pane.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
};
use gpui_kit::{
    App, ClickEvent, FocusHandle, IntoElement, Rems, RenderOnce, SharedString, Window, div,
    prelude::*, rems,
};

use super::icon::AppIcon;

const ITEM_HEIGHT: Rems = rems(2.25);
const PILL_WIDTH: Rems = rems(0.1875);
const PILL_HEIGHT: Rems = rems(1.);
/// The largest count shown in full. A larger count shows as "999+".
const MAX_SHOWN_UNREAD: u32 = 999;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// How an item shows its icon and label.
enum Presentation {
    /// The icon leads the label, which fills the item.
    Labeled,
    /// The icon fills the item and the label becomes its accessible name and tooltip.
    IconOnly,
}

/// A destination or folder in the navigation pane.
///
/// The selected item gets a filled surface. A labeled item also gets a short accent pill, while
/// an icon-only item shows selection with the fill alone. The pill is an overlay without
/// listeners, so pointer events reach the button beneath it.
#[derive(IntoElement)]
pub(crate) struct NavItem {
    id: SharedString,
    label: SharedString,
    icon: AppIcon,
    presentation: Presentation,
    selected: bool,
    unread_count: Option<u32>,
    unavailable: Option<&'static str>,
    focus: Option<FocusHandle>,
    on_click: Option<ClickHandler>,
}

impl NavItem {
    /// An item that shows its icon and label.
    pub(crate) fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: AppIcon,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon,
            presentation: Presentation::Labeled,
            selected: false,
            unread_count: None,
            unavailable: None,
            focus: None,
            on_click: None,
        }
    }

    /// An item that shows only its icon. The label names it for assistive technology and a tooltip.
    pub(crate) fn icon_only(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: AppIcon,
    ) -> Self {
        Self {
            presentation: Presentation::IconOnly,
            ..Self::new(id, label, icon)
        }
    }

    /// Marks the item as the current destination.
    pub(crate) fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Shows how many messages are unread, when the item is labeled and the count is above 0.
    ///
    /// The count trails the label, and the accessible name carries the exact number, as in
    /// "Inbox, 12 unread".
    pub(crate) fn unread_count(mut self, count: Option<u32>) -> Self {
        self.unread_count = count;
        self
    }

    /// Disables the item and shows `reason` as its tooltip when it is `Some`.
    pub(crate) fn unavailable(mut self, reason: Option<&'static str>) -> Self {
        self.unavailable = reason;
        self
    }

    /// Uses a handle the caller owns, so it can focus the item and place it in the tab order.
    pub(crate) fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// Runs `handler` when the item is activated by pointer or keyboard.
    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// The unread count when the item shows one. An icon-only item has no room for it.
    fn shown_unread_count(&self) -> Option<u32> {
        self.unread_count
            .filter(|count| *count > 0 && matches!(self.presentation, Presentation::Labeled))
    }

    /// The name assistive technology reads: the label, plus the exact unread count when shown.
    fn accessible_label(&self) -> SharedString {
        match self.shown_unread_count() {
            Some(count) => format!("{}, {count} unread", self.label).into(),
            None => self.label.clone(),
        }
    }

    fn render_button(&self, cx: &App) -> Button {
        let selector = self.id.to_string();
        let button = Button::new(self.id.clone())
            .debug_selector(move || selector)
            .ghost()
            .selected(self.selected)
            .disabled(self.unavailable.is_some())
            .accessibility_label(self.accessible_label())
            .w_full()
            .h(ITEM_HEIGHT);
        match self.presentation {
            Presentation::Labeled => button.px_3().child(self.render_labeled_content(cx)),
            Presentation::IconOnly => button.icon(Icon::new(self.icon)),
        }
    }

    fn render_labeled_content(&self, cx: &App) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_3()
            .child(Icon::new(self.icon).size_4().flex_shrink_0())
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .when(self.selected, |label| label.font_semibold())
                    .child(self.label.clone()),
            )
            .children(
                self.shown_unread_count()
                    .map(|count| self.render_count(count, cx)),
            )
    }

    /// The count is muted, except on the selected item, where the light theme's muted text falls
    /// under 4.5:1 against the selection fill.
    fn render_count(&self, count: u32, cx: &App) -> impl IntoElement {
        let selector = format!("{}-unread", self.id);
        let theme = cx.theme();
        let color = if self.selected {
            theme.foreground
        } else {
            theme.muted_foreground
        };
        div()
            .debug_selector(move || selector)
            .flex_shrink_0()
            .text_xs()
            .text_color(color)
            .child(unread_count_text(count))
    }

    fn tooltip(&self) -> Option<SharedString> {
        match (self.unavailable, &self.presentation) {
            (Some(reason), _) => Some(reason.into()),
            (None, Presentation::IconOnly) => Some(self.label.clone()),
            (None, Presentation::Labeled) => None,
        }
    }

    fn shows_pill(&self) -> bool {
        matches!(self.presentation, Presentation::Labeled)
    }

    fn render_pill(&self, cx: &App) -> impl IntoElement {
        let selector = format!("{}-pill", self.id);
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_1()
            .flex()
            .items_center()
            .child(
                div()
                    .debug_selector(move || selector)
                    .w(PILL_WIDTH)
                    .h(PILL_HEIGHT)
                    .rounded_full()
                    .bg(cx.theme().primary),
            )
    }
}

/// The count as the item shows it: the number, or "999+" above [`MAX_SHOWN_UNREAD`].
fn unread_count_text(count: u32) -> String {
    if count > MAX_SHOWN_UNREAD {
        format!("{MAX_SHOWN_UNREAD}+")
    } else {
        count.to_string()
    }
}

impl RenderOnce for NavItem {
    fn render(mut self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_click = self.on_click.take();
        let focus = self.focus.take();
        let button = self
            .render_button(cx)
            .when_some(self.tooltip(), |button, tooltip| button.tooltip(tooltip))
            .when_some(focus, |button, focus| button.track_focus(&focus))
            .when_some(on_click, |button, on_click| button.on_click(on_click));
        div()
            .relative()
            .map(|wrapper| match self.presentation {
                Presentation::Labeled => wrapper.w_full(),
                Presentation::IconOnly => wrapper.flex_1().min_w_0(),
            })
            .child(button)
            .when(self.selected && self.shows_pill(), |wrapper| {
                wrapper.child(self.render_pill(cx))
            })
    }
}

#[cfg(test)]
mod tests;
