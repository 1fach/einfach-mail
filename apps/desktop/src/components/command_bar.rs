//! The command bar: the actions of the open item as one row of ghost buttons.

use std::rc::Rc;

use gpui_kit::base::Toolbar;
use gpui_kit::component::{
    Disableable as _, Icon,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, IntoElement, RenderOnce, SharedString, Window, prelude::*,
};

use super::icon::AppIcon;

const MORE_ACTIONS_LABEL: &str = "More actions";

type InvokeHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type MenuBuilder = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// Whether a command stays on the bar when the bar is short of room.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandPriority {
    Primary,
    Secondary,
}

enum CommandKind {
    /// Runs the handler. Only this kind can be secondary.
    Invoke {
        handler: InvokeHandler,
        priority: CommandPriority,
    },
    /// Opens a menu. It needs its own trigger button, so it always stays on the bar.
    Menu(MenuBuilder),
}

/// One action on a [`CommandBar`].
pub(crate) struct Command {
    id: &'static str,
    label: SharedString,
    icon: AppIcon,
    disabled: bool,
    loading: bool,
    kind: CommandKind,
}

impl Command {
    /// A command that runs `handler` when activated.
    pub(crate) fn invoke(
        id: &'static str,
        label: impl Into<SharedString>,
        icon: AppIcon,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self::new(
            id,
            label,
            icon,
            CommandKind::Invoke {
                handler: Rc::new(handler),
                priority: CommandPriority::Primary,
            },
        )
    }

    /// A command that opens the menu `builder` fills.
    pub(crate) fn menu(
        id: &'static str,
        label: impl Into<SharedString>,
        icon: AppIcon,
        builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        Self::new(id, label, icon, CommandKind::Menu(Rc::new(builder)))
    }

    fn new(
        id: &'static str,
        label: impl Into<SharedString>,
        icon: AppIcon,
        kind: CommandKind,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            icon,
            disabled: false,
            loading: false,
            kind,
        }
    }

    /// Moves the command into the More menu when the bar is short of room. A menu command ignores
    /// this, because its trigger cannot live inside another menu.
    pub(crate) fn secondary(mut self) -> Self {
        if let CommandKind::Invoke { priority, .. } = &mut self.kind {
            *priority = CommandPriority::Secondary;
        }
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Shows the command as busy and blocks it, so its label should say what is in progress.
    pub(crate) fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    fn priority(&self) -> CommandPriority {
        match &self.kind {
            CommandKind::Invoke { priority, .. } => *priority,
            CommandKind::Menu(_) => CommandPriority::Primary,
        }
    }

    fn render_button(self) -> AnyElement {
        let id = self.id;
        let button = Button::new(id)
            .debug_selector(|| id.to_owned())
            .ghost()
            .icon(Icon::new(self.icon))
            .label(self.label)
            .loading(self.loading)
            .disabled(self.disabled);
        match self.kind {
            CommandKind::Invoke { handler, .. } => button
                .on_click(move |event, window, cx| handler(event, window, cx))
                .into_any_element(),
            CommandKind::Menu(builder) => button
                .dropdown_menu(move |menu, window, cx| builder(menu, window, cx))
                .into_any_element(),
        }
    }

    /// The menu entry for a command that moved into the More menu. A busy command is disabled
    /// there, because a menu item has no busy state of its own.
    fn folded_item(&self) -> Option<FoldedItem> {
        let CommandKind::Invoke { handler, .. } = &self.kind else {
            return None;
        };
        Some(FoldedItem {
            label: self.label.clone(),
            icon: self.icon,
            disabled: self.disabled || self.loading,
            handler: Rc::clone(handler),
        })
    }
}

struct FoldedItem {
    label: SharedString,
    icon: AppIcon,
    disabled: bool,
    handler: InvokeHandler,
}

impl FoldedItem {
    fn menu_item(&self) -> PopupMenuItem {
        let handler = Rc::clone(&self.handler);
        PopupMenuItem::new(self.label.clone())
            .icon(Icon::new(self.icon))
            .disabled(self.disabled)
            .on_click(move |event, window, cx| handler(event, window, cx))
    }
}

/// A row of commands for the item in view.
///
/// Every command is a ghost button with an icon and a label. When `overflow` is on, the secondary
/// commands collapse into one More menu that keeps their enabled and busy state. The bar carries
/// the toolbar role, so arrow keys move focus along it.
#[derive(IntoElement)]
pub(crate) struct CommandBar {
    id: &'static str,
    label: SharedString,
    overflow: bool,
    commands: Vec<Command>,
}

impl CommandBar {
    /// A bar whose `label` names it for assistive technology.
    pub(crate) fn new(id: &'static str, label: impl Into<SharedString>) -> Self {
        Self {
            id,
            label: label.into(),
            overflow: false,
            commands: Vec::new(),
        }
    }

    pub(crate) fn overflow(mut self, overflow: bool) -> Self {
        self.overflow = overflow;
        self
    }

    pub(crate) fn commands(mut self, commands: impl IntoIterator<Item = Command>) -> Self {
        self.commands.extend(commands);
        self
    }

    fn render_more_button(&self, folded: Vec<FoldedItem>) -> AnyElement {
        let more_id = SharedString::from(format!("{}-more", self.id));
        let selector = more_id.to_string();
        let folded = Rc::new(folded);
        Button::new(more_id)
            .debug_selector(move || selector)
            .ghost()
            .icon(Icon::new(AppIcon::More))
            .accessibility_label(MORE_ACTIONS_LABEL)
            .tooltip(MORE_ACTIONS_LABEL)
            .dropdown_menu(move |menu, _, _| {
                folded
                    .iter()
                    .fold(menu, |menu, item| menu.item(item.menu_item()))
            })
            .into_any_element()
    }
}

impl RenderOnce for CommandBar {
    fn render(mut self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let commands = std::mem::take(&mut self.commands);
        let (kept, folded): (Vec<_>, Vec<_>) = commands
            .into_iter()
            .partition(|command| !self.overflow || command.priority() == CommandPriority::Primary);
        let folded: Vec<_> = folded.iter().filter_map(Command::folded_item).collect();
        let more = (!folded.is_empty()).then(|| self.render_more_button(folded));
        Toolbar::new(self.id)
            .aria_label(self.label.clone())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .min_w_0()
            .children(kept.into_iter().map(Command::render_button))
            .children(more)
    }
}

#[cfg(test)]
mod tests;
