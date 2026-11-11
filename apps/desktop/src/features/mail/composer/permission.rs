use std::sync::Arc;

use gpui_kit::{AsyncApp, Context};

use super::{ComposerStatus, MailComposer};

impl MailComposer {
    pub(super) fn request_permission(&mut self, cx: &mut Context<Self>) {
        let permission_requester = Arc::clone(&self.permission_requester);
        let account_id = self.account_id.clone();
        self.status = ComposerStatus::RenewingPermission;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = permission_requester
                .request_send_permission(&account_id)
                .await;
            let _ = this.update(cx, |composer, cx| {
                composer.task = None;
                composer.status = match result {
                    Ok(()) => ComposerStatus::PermissionUpdated,
                    Err(error) => ComposerStatus::PermissionRenewalFailed(error),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }
}
