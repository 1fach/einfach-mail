use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use gpui_kit::{Modifiers, TestAppContext, px, size};

use super::*;
use crate::test_support::account::*;
use einfach_mail_application::{
    AccountCacheOpenError, AccountCacheRemovalError, AccountOperationError,
    AccountPermissionRequest, AccountProviderKind, AccountSession, AccountSessionFactory,
    AccountWorkflow,
};
use einfach_mail_domain::{
    AccountCapabilities, AccountId, AccountProfile, CalendarAction, CalendarCapabilities,
    MailAction, MailCapabilities,
};

mod account_setup;
mod account_state;
mod permission_review;
mod status_copy;
