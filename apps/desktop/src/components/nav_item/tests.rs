use std::{cell::Cell, rc::Rc};

use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{
    App, ClickEvent, Context, IntoElement, Modifiers, Render, Role, TestAppContext,
    VisualTestContext, Window, div, prelude::*, px,
};

use super::{AppIcon, NavItem, PILL_HEIGHT, PILL_WIDTH};
use crate::components::NAVIGATION_PANE_WIDTH;

const UNAVAILABLE_REASON: &str = "Wait for the current update to finish.";

struct NavItemHarness {
    activations: Rc<Cell<usize>>,
}

impl Render for NavItemHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let count_activation = |activations: &Rc<Cell<usize>>| {
            let activations = Rc::clone(activations);
            move |_: &ClickEvent, _: &mut Window, _: &mut App| {
                activations.set(activations.get() + 1)
            }
        };
        div()
            .w(NAVIGATION_PANE_WIDTH)
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(
                NavItem::new("nav-selected", "Inbox", AppIcon::Folder)
                    .selected(true)
                    .on_click(count_activation(&self.activations)),
            )
            .child(NavItem::new("nav-idle", "Archive", AppIcon::Folder))
            .child(
                NavItem::new("nav-blocked", "Drafts", AppIcon::Folder)
                    .unavailable(Some(UNAVAILABLE_REASON))
                    .on_click(count_activation(&self.activations)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        NavItem::icon_only("nav-icon-selected", "Settings", AppIcon::Settings)
                            .selected(true),
                    )
                    .child(NavItem::icon_only(
                        "nav-icon-idle",
                        "Calendar",
                        AppIcon::Calendar,
                    )),
            )
    }
}

fn open_harness(cx: &mut TestAppContext) -> (&mut VisualTestContext, Rc<Cell<usize>>) {
    cx.update(gpui_kit::init);
    let activations = Rc::new(Cell::new(0));
    let harness_activations = Rc::clone(&activations);
    let (_, context) = cx.add_window_view(move |window, cx| {
        let harness = cx.new(|_| NavItemHarness {
            activations: harness_activations,
        });
        gpui_kit::base::Root::new(harness, window, cx)
    });
    context.run_until_parked();
    context.update(|window, app| {
        let _ = window.draw(app);
    });
    (context, activations)
}

#[gpui_kit::test]
fn the_selected_labeled_item_shows_a_centered_accent_pill(cx: &mut TestAppContext) {
    let (context, _) = open_harness(cx);
    let rem_size = context.update(|window, _| window.rem_size());
    let item = context
        .debug_bounds("nav-selected")
        .expect("the selected item is rendered");
    let pill = context
        .debug_bounds("nav-selected-pill")
        .expect("the selected item shows a pill");

    assert_eq!(pill.size.width, PILL_WIDTH.to_pixels(rem_size));
    assert_eq!(pill.size.height, PILL_HEIGHT.to_pixels(rem_size));
    assert!(
        (pill.center().y - item.center().y).abs() < px(0.5),
        "pill {pill:?} is vertically centered on its item {item:?}"
    );
    assert!(pill.left() > item.left() && pill.right() < item.right());
    assert!(pill.top() > item.top() && pill.bottom() < item.bottom());
}

#[gpui_kit::test]
fn a_selected_icon_only_item_shows_selection_without_a_pill(cx: &mut TestAppContext) {
    let (context, _) = open_harness(cx);

    assert!(
        context.debug_bounds("nav-icon-selected").is_some(),
        "the selected icon-only item is rendered"
    );
    assert!(
        context.debug_bounds("nav-icon-selected-pill").is_none(),
        "an icon-only item has no pill"
    );
}

#[gpui_kit::test]
fn an_unselected_item_has_no_pill(cx: &mut TestAppContext) {
    let (context, _) = open_harness(cx);

    for pill in ["nav-idle-pill", "nav-blocked-pill", "nav-icon-idle-pill"] {
        assert!(context.debug_bounds(pill).is_none(), "{pill} is absent");
    }
}

#[gpui_kit::test]
fn pointer_hits_on_the_pill_reach_the_item(cx: &mut TestAppContext) {
    let (context, activations) = open_harness(cx);
    let pill = context
        .debug_bounds("nav-selected-pill")
        .expect("the selected item shows a pill");

    context.simulate_click(pill.center(), Modifiers::default());
    context.run_until_parked();

    assert_eq!(activations.get(), 1);
}

#[gpui_kit::test]
fn an_icon_only_item_is_named_and_described_by_its_label(cx: &mut TestAppContext) {
    let (context, _) = open_harness(cx);

    context.update(|window, _| {
        for label in ["Settings", "Calendar"] {
            let item = window.find_by_label(label);
            assert_eq!(item.role(), Some(Role::Button));
            assert_eq!(item.description(), Some(label));
        }
    });
}

#[gpui_kit::test]
fn an_unavailable_item_is_disabled_and_explains_why(cx: &mut TestAppContext) {
    let (context, activations) = open_harness(cx);
    let blocked = context
        .debug_bounds("nav-blocked")
        .expect("the unavailable item is rendered");

    context.simulate_click(blocked.center(), Modifiers::default());
    context.run_until_parked();

    context.update(|window, _| {
        let item = window.find_by_label("Drafts");
        assert_eq!(item.disabled(), Some(true));
        assert_eq!(item.description(), Some(UNAVAILABLE_REASON));
    });
    assert_eq!(activations.get(), 0);
}
