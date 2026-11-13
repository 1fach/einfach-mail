use std::{cell::RefCell, rc::Rc};

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{
    App, ClickEvent, Context, IntoElement, Modifiers, Pixels, Point, Render, Role, TestAppContext,
    VisualTestContext, Window, div, prelude::*,
};

use super::{AppIcon, Command, CommandBar};

const MORE_ID: &str = "test-bar-more";

type Activations = Rc<RefCell<Vec<&'static str>>>;

struct CommandBarHarness {
    overflow: bool,
    activations: Activations,
}

fn recording(
    activations: &Activations,
    id: &'static str,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let activations = Rc::clone(activations);
    move |_, _, _| activations.borrow_mut().push(id)
}

impl Render for CommandBarHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let activations = &self.activations;
        let moved = Rc::clone(activations);
        div().w_full().child(
            CommandBar::new("test-bar", "Test actions")
                .overflow(self.overflow)
                .commands([
                    Command::invoke(
                        "cmd-reply",
                        "Reply",
                        AppIcon::Reply,
                        recording(activations, "reply"),
                    ),
                    Command::menu(
                        "cmd-move",
                        "Move to…",
                        AppIcon::Move,
                        move |menu, _, _| {
                            let moved = Rc::clone(&moved);
                            menu.item(
                                PopupMenuItem::new("Archive")
                                    .on_click(move |_, _, _| moved.borrow_mut().push("moved")),
                            )
                        },
                    )
                    .secondary(),
                    Command::invoke(
                        "cmd-read",
                        "Mark read",
                        AppIcon::MarkRead,
                        recording(activations, "read"),
                    )
                    .secondary()
                    .disabled(true),
                    Command::invoke(
                        "cmd-flag",
                        "Updating…",
                        AppIcon::Flag,
                        recording(activations, "flag"),
                    )
                    .secondary()
                    .loading(true),
                    Command::invoke(
                        "cmd-clear",
                        "Clear flag",
                        AppIcon::Unflag,
                        recording(activations, "clear"),
                    )
                    .secondary(),
                ]),
        )
    }
}

fn open_bar(cx: &mut TestAppContext, overflow: bool) -> (&mut VisualTestContext, Activations) {
    cx.update(gpui_kit::init);
    let activations = Activations::default();
    let harness_activations = Rc::clone(&activations);
    let (_, context) = cx.add_window_view(move |window, cx| {
        let harness = cx.new(|_| CommandBarHarness {
            overflow,
            activations: harness_activations,
        });
        gpui_kit::base::Root::new(harness, window, cx)
    });
    redraw(context);
    (context, activations)
}

fn redraw(context: &mut VisualTestContext) {
    context.run_until_parked();
    context.update(|window, app| {
        let _ = window.draw(app);
    });
}

fn click(context: &mut VisualTestContext, selector: &'static str) {
    let bounds = context
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is rendered"));
    context.simulate_click(bounds.center(), Modifiers::default());
    redraw(context);
}

struct MenuEntry {
    label: String,
    center: Point<Pixels>,
}

fn menu_entries(context: &mut VisualTestContext) -> Vec<MenuEntry> {
    context.update(|window, _| {
        window
            .find_all_by_role(Role::MenuItem)
            .iter()
            .map(|item| MenuEntry {
                label: item.label().unwrap_or_default().to_owned(),
                center: item.bounds().center(),
            })
            .collect()
    })
}

fn open_more_menu(context: &mut VisualTestContext) -> Vec<MenuEntry> {
    if menu_entries(context).is_empty() {
        click(context, MORE_ID);
    }
    menu_entries(context)
}

#[gpui_kit::test]
fn the_bar_is_a_labeled_toolbar_of_ghost_buttons_with_every_command(cx: &mut TestAppContext) {
    let (context, _) = open_bar(cx, false);

    let toolbars = context.update(|window, _| window.find_all_by_role(Role::Toolbar));
    assert_eq!(toolbars.len(), 1);
    assert_eq!(toolbars[0].label(), Some("Test actions"));
    for command in ["cmd-reply", "cmd-move", "cmd-read", "cmd-flag", "cmd-clear"] {
        assert!(
            context.debug_bounds(command).is_some(),
            "{command} is a button on the expanded bar"
        );
    }
    assert!(
        context.debug_bounds(MORE_ID).is_none(),
        "the expanded bar has no More menu"
    );
}

#[gpui_kit::test]
fn overflow_folds_secondary_commands_into_the_more_menu_and_keeps_primary_ones(
    cx: &mut TestAppContext,
) {
    let (context, _) = open_bar(cx, true);

    assert!(context.debug_bounds("cmd-reply").is_some());
    assert!(
        context.debug_bounds("cmd-move").is_some(),
        "a menu command stays on the bar even when marked secondary"
    );
    for folded in ["cmd-read", "cmd-flag", "cmd-clear"] {
        assert!(
            context.debug_bounds(folded).is_none(),
            "{folded} left the bar"
        );
    }
    assert!(context.update(|window, _| window.find_by_label("More actions").visible()));
}

#[gpui_kit::test]
fn the_more_menu_lists_the_folded_commands_in_order_with_their_busy_labels(
    cx: &mut TestAppContext,
) {
    let (context, _) = open_bar(cx, true);

    let items = open_more_menu(context);

    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, ["Mark read", "Updating…", "Clear flag"]);
}

#[gpui_kit::test]
fn menu_items_activate_exactly_when_the_expanded_buttons_do(cx: &mut TestAppContext) {
    let (expanded, expanded_activations) = open_bar(cx, false);
    for command in ["cmd-read", "cmd-flag", "cmd-clear"] {
        click(expanded, command);
    }
    let from_buttons = expanded_activations.borrow().clone();

    let (overflowed, overflow_activations) = open_bar(cx, true);
    for index in 0..3 {
        let entries = open_more_menu(overflowed);
        overflowed.simulate_click(entries[index].center, Modifiers::default());
        redraw(overflowed);
    }

    assert_eq!(
        from_buttons,
        ["clear"],
        "only the enabled, idle button activates"
    );
    assert_eq!(*overflow_activations.borrow(), from_buttons);
}

#[gpui_kit::test]
fn a_primary_menu_command_opens_its_menu(cx: &mut TestAppContext) {
    let (context, activations) = open_bar(cx, true);

    click(context, "cmd-move");
    let archive = context.update(|window, _| window.find_by_label("Archive").bounds().center());
    context.simulate_click(archive, Modifiers::default());
    redraw(context);

    assert_eq!(*activations.borrow(), ["moved"]);
}
