use crate::app::preview::PreviewDestination;

use super::*;

type WorkspaceTestView = gpui_kit::Entity<Workspace<TestProvider>>;

fn workspace_waiting_for_account(
    cx: &mut TestAppContext,
    destination: PreviewDestination,
) -> (WorkspaceTestView, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let workflow = Arc::new(AccountWorkflow::new(TestProvider::default()));
    let factory = unused_account_session_factory();
    let (view, context) = cx.add_window_view(move |_, cx| {
        let mut workspace = Workspace::new(Ok(AccountServices::new(workflow, factory)), cx);
        workspace.start_at_preview_destination(destination);
        workspace
    });
    context.run_until_parked();
    (view, context)
}

fn connect_account(view: &WorkspaceTestView, context: &mut VisualTestContext) {
    context.cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.test_set_connected_account(connected_account(mail_only_profile()), cx);
        });
    });
    context.run_until_parked();
}

fn content(view: &WorkspaceTestView, context: &mut VisualTestContext) -> WorkspaceTestContent {
    context.cx.update(|cx| view.read(cx).test_content())
}

#[gpui_kit::test]
fn preview_starts_on_settings_after_the_account_view_updates(cx: &mut TestAppContext) {
    let (view, context) = workspace_waiting_for_account(cx, PreviewDestination::Settings);

    connect_account(&view, context);

    assert_eq!(content(&view, context), WorkspaceTestContent::Settings);
}

#[gpui_kit::test]
fn preview_waits_for_the_restored_account_before_opening_mail(cx: &mut TestAppContext) {
    let (view, context) = workspace_waiting_for_account(cx, PreviewDestination::Mail);
    assert_eq!(content(&view, context), WorkspaceTestContent::Accounts);

    connect_account(&view, context);

    assert_eq!(content(&view, context), WorkspaceTestContent::Mail);
}

#[gpui_kit::test]
fn preview_start_destination_applies_only_once(cx: &mut TestAppContext) {
    let (view, context) = workspace_waiting_for_account(cx, PreviewDestination::MailReader);

    connect_account(&view, context);
    assert_eq!(content(&view, context), WorkspaceTestContent::Mail);
    click(context, "nav-settings");
    click(context, "nav-accounts");

    assert_eq!(content(&view, context), WorkspaceTestContent::Accounts);
}

#[gpui_kit::test]
fn preview_keeps_the_accounts_view_when_asked_for_it(cx: &mut TestAppContext) {
    let (view, context) = workspace_waiting_for_account(cx, PreviewDestination::Accounts);

    connect_account(&view, context);

    assert_eq!(content(&view, context), WorkspaceTestContent::Accounts);
}
