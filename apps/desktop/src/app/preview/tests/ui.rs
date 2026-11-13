use super::super::{PreviewScenario, account_services, preview_account};
use crate::app::workspace::{Workspace, WorkspaceTestSection};
use crate::features::AppearanceSettingsStore;
use crate::test_support::wait_for_ui_condition;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Role, TestAppContext, px, size};

#[gpui_kit::test]
async fn mail_sync_recovery_preview_retries_at_wide_width(cx: &mut TestAppContext) {
    assert_recovery_retry_at_width(cx, 1280.).await;
}

#[gpui_kit::test]
async fn mail_sync_recovery_preview_retries_at_compact_width(cx: &mut TestAppContext) {
    assert_recovery_retry_at_width(cx, 640.).await;
}

async fn assert_recovery_retry_at_width(cx: &mut TestAppContext, width: f32) {
    cx.update(gpui_kit::init);
    let settings_directory =
        tempfile::tempdir().expect("temporary appearance settings directory is created");
    let settings_store =
        AppearanceSettingsStore::at_path(settings_directory.path().join("preferences.json"));
    let account_id = preview_account().profile().id().clone();
    let services = account_services(PreviewScenario::MailSyncRecovery);
    let (workspace, context) = cx.add_window_view(move |window, cx| {
        Workspace::new_for_window(Ok(services), Ok(settings_store), window, cx)
    });
    context.simulate_resize(size(px(width), px(800.)));

    wait_for_ui_condition(context, |_, app| {
        workspace
            .read(app)
            .test_status_is_connected_to(&account_id, app)
    })
    .await;

    context.update(|window, cx| window.click("nav-mail", cx));
    context.run_until_parked();

    let (failure_role, failure_status) = context.update(|window, _| {
        let status = window.find("mail-sync-status");
        (status.role(), status.label().map(str::to_owned))
    });
    assert_eq!(
        failure_role,
        Some(Role::Alert),
        "mail sync failure is announced as an alert: {failure_status:?}"
    );
    assert!(
        failure_status
            .as_deref()
            .is_some_and(|status| status.starts_with("Couldn't complete this mail request.")),
        "synthetic sync failure appears in the Mail status: {failure_status:?}"
    );
    assert!(
        context
            .debug_bounds("message-row-preview-message-1")
            .is_some(),
        "cached synthetic mail remains visible after the sync failure"
    );

    let (retry, retry_visible, viewport) = context.update(|window, _| {
        let retry = window.find("sync-mail");
        (retry.bounds(), retry.visible(), window.bounds())
    });
    assert!(
        retry_visible,
        "Sync action bounds {retry:?} must be visible in {viewport:?}"
    );
    context.update(|window, cx| window.click("sync-mail", cx));
    context.run_until_parked();

    let (retry_role, retry_status) = context.update(|window, _| {
        let status = window.find("mail-sync-status");
        (status.role(), status.label().map(str::to_owned))
    });
    assert_eq!(
        retry_role,
        Some(Role::Status),
        "successful retry is announced as status: {retry_status:?}"
    );
    assert!(
        retry_status
            .as_deref()
            .is_some_and(|status| status.starts_with("Up to date")),
        "successful retry reports the updated mail state: {retry_status:?}"
    );

    assert!(context.cx.update(|app| {
        workspace.read(app).test_selected_section() == WorkspaceTestSection::Mail
    }));
    assert!(
        context
            .debug_bounds("message-row-preview-message-1")
            .is_some()
    );
}
