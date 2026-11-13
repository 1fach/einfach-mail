use einfach_mail_application::{
    AccountCacheRemovalError, AccountSessionFuture, CalendarSession, MailSession, MailWorkflow,
};
use gpui_kit::Role;
use gpui_kit::test::TestWindowExt as _;

use super::*;
use crate::test_support::mail::{OfflineProvider, fixture};
use crate::test_support::wait_for_ui_condition;

const DESTINATIONS: [(&str, &str, &str); 4] = [
    ("Mail", "nav-mail", "nav-mail-pill"),
    ("Calendar", "nav-calendar", "nav-calendar-pill"),
    ("Accounts", "nav-accounts", "nav-accounts-pill"),
    ("Settings", "nav-settings", "nav-settings-pill"),
];

fn full_access_profile() -> AccountProfile {
    synthetic_account_profile(
        synthetic_account_id(),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid")
}

fn connect(view: &WorkspaceTestView, profile: AccountProfile, context: &mut VisualTestContext) {
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.test_set_connected_account(connected_account(profile), cx);
            cx.notify();
        });
    });
    context.run_until_parked();
    draw_account_view(context);
}

type WorkspaceTestView = gpui_kit::Entity<Workspace<TestProvider>>;

#[gpui_kit::test]
fn the_destination_switcher_names_each_destination_and_marks_the_current_one(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) =
        cx.add_window_view(|_, cx| Workspace::new(Ok(AccountServices::new(workflow, factory)), cx));
    connect(&view, full_access_profile(), context);

    let (identity, switcher, items) = context.update(|window, _| {
        let items = DESTINATIONS.map(|(label, id, pill)| {
            let item = window.find_by_label(label);
            assert_eq!(item.role(), Some(Role::Button), "{label} is a button");
            assert_eq!(item.description(), Some(label), "{label} has a tooltip");
            assert_ne!(item.disabled(), Some(true), "{label} is available");
            (label, id, pill, item.bounds())
        });
        (
            window.find_by_label("Active account: Ada Example").bounds(),
            window.find_by_label("Destinations").bounds(),
            items,
        )
    });

    for (label, id, pill, bounds) in items {
        let shows_pill = context.debug_bounds(pill).is_some();
        assert_eq!(context.debug_bounds(id), Some(bounds), "{label} keeps {id}");
        assert_eq!(
            shows_pill,
            label == "Accounts",
            "only the current destination shows the selection pill, got {label}: {shows_pill}"
        );
        assert!(bounds.top() >= switcher.top() && bounds.bottom() <= switcher.bottom());
        assert!(bounds.top() > identity.bottom());
    }
    assert!(
        items
            .windows(2)
            .all(|pair| pair[0].3.right() <= pair[1].3.left() && pair[0].3.top() == pair[1].3.top()),
        "the destinations sit in one row in menu order"
    );
}

struct FolderAccountSession {
    mail: Arc<dyn MailSession>,
}

impl AccountSession for FolderAccountSession {
    fn mail_session(&self) -> Arc<dyn MailSession> {
        Arc::clone(&self.mail)
    }

    fn calendar_session(&self) -> Arc<dyn CalendarSession> {
        Arc::new(TestCalendarSession)
    }

    fn remove_account_cache(
        &self,
    ) -> AccountSessionFuture<'_, Result<(), AccountCacheRemovalError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

#[gpui_kit::test]
async fn tab_order_follows_the_pane_from_top_to_bottom(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (profile, cache, _) = fixture(synthetic_account_id());
    let mail: Arc<dyn MailSession> = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let factory: Arc<dyn AccountSessionFactory> = Arc::new(TestAccountSessionFactory {
        session: Arc::new(FolderAccountSession { mail }),
        fallback_removals: Arc::new(AtomicBool::new(false)),
        session_open_attempts: Arc::new(AtomicUsize::new(0)),
        opened_account_ids: Arc::new(Mutex::new(Vec::new())),
    });
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let (view, context) =
        cx.add_window_view(|_, cx| Workspace::new(Ok(AccountServices::new(workflow, factory)), cx));
    connect(&view, profile, context);
    click(context, "nav-mail");
    wait_for_ui_condition(context, |window, _| {
        window.try_find("folder:inbox").is_some()
            && window.find_by_label("New mail").disabled() != Some(true)
    })
    .await;

    for label in [
        "New mail", "Inbox", "Archive", "Mail", "Calendar", "Accounts", "Settings",
    ] {
        context.update(|window, cx| window.focus_next(cx));
        let focused = context.update(|window, _| window.find_by_label(label).focused());
        assert_eq!(focused, Some(true), "Tab reaches {label} in pane order");
    }
}
