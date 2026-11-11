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
use einfach_mail_domain::{AccountId, AccountProfile};
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{Background, Hsla, TestAppContext, VisualTestContext, px};

use crate::features::{
    AccountServices, AccountView, AccountViewTestContent, AccountViewTestSection,
    NavigationSurfaceAppearance,
};
use crate::test_support::account::*;

mod appearance;
mod navigation;
mod session_lifecycle;
