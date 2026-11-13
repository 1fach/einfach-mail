//! The product icon set: one Lucide family, embedded as a small named subset.
//!
//! Feature code names an [`AppIcon`] and never a Lucide file. `AppAssets` serves the icons
//! `AppIcon` uses and falls back to GPUI Kit's default bundle for the icons GPUI components draw.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, IconName, IconNamed, icon_assets};
use gpui_kit::{AssetSource, SharedString};

/// Declares `AppIcon` and embeds exactly the Lucide icons its variants name.
macro_rules! app_icons {
    ($($variant:ident => $lucide:ident),+ $(,)?) => {
        /// An icon the product shows. Every variant maps to one embedded Lucide icon.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[cfg_attr(
            not(test),
            expect(dead_code, reason = "the Fluent primitives are the first consumers")
        )]
        pub(crate) enum AppIcon {
            $($variant),+
        }

        #[cfg(test)]
        impl AppIcon {
            const ALL: &'static [AppIcon] = &[$(AppIcon::$variant),+];
        }

        impl IconNamed for AppIcon {
            fn path(self) -> SharedString {
                match self {
                    $(AppIcon::$variant => IconName::$lucide),+
                }
                .path()
            }
        }

        icon_assets!(AppIcons, [$($lucide),+]);
    };
}

app_icons! {
    Mail => Mail,
    Calendar => Calendar,
    Accounts => CircleUser,
    Settings => Settings,
    NewMail => SquarePen,
    Folder => Folder,
    Reply => Reply,
    Archive => Archive,
    Move => FolderInput,
    Delete => Trash,
    MarkRead => MailOpen,
    MarkUnread => Mail,
    Flag => Flag,
    Unflag => FlagOff,
    Sync => RefreshCw,
    More => Ellipsis,
    Back => ArrowLeft,
    Appearance => Palette,
    Info => Info,
    Warning => TriangleAlert,
    Error => CircleX,
    NarrowList => PanelLeftClose,
    WidenList => PanelLeftOpen,
    Success => CircleCheck,
    Permissions => ShieldCheck,
    Server => Server,
}

/// The application's asset source: the `AppIcon` subset first, then GPUI Kit's default icons.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        match AppIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(AppIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests;
