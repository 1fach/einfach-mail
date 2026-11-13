use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use tokio::sync::oneshot;

use einfach_mail_application::{
    AccountCacheOpenError, AccountOperationError, AccountSession, AccountSessionFactory,
    AccountWorkflow,
};
use einfach_mail_domain::{AccountCapabilities, AccountId, AccountProfile};
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{Background, Hsla, TestAppContext, VisualTestContext, px};

use crate::features::AccountServices;
use crate::test_support::account::*;

use super::{NavigationSurfaceAppearance, Workspace, WorkspaceTestContent, WorkspaceTestSection};

mod appearance;
mod calendar_navigation;
mod destination_switcher;
mod navigation;
#[cfg(feature = "ui-preview")]
mod preview_start;
mod session_lifecycle;
