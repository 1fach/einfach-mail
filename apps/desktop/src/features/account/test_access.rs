use einfach_mail_application::{
    AccountOperationError, AccountPermissionRequest, AccountProvider, ConnectedAccount,
};
use einfach_mail_domain::AccountId;

use super::{AccountStatus, AccountView, SavedCredentialState};

impl<P: AccountProvider + 'static> AccountView<P> {
    pub(crate) fn test_set_connected_account(&mut self, account: ConnectedAccount) {
        self.status = AccountStatus::Connected(account);
    }

    pub(crate) fn test_status_is_connected_to(&self, account_id: &AccountId) -> bool {
        matches!(
            &self.status,
            AccountStatus::Connected(account) if account.profile().id() == account_id
        )
    }

    pub(crate) fn test_status_is_disconnected(&self) -> bool {
        matches!(self.status, AccountStatus::Disconnected)
    }

    pub(crate) fn test_status_is_failed_with_possible_saved_credential(
        &self,
        expected_error: AccountOperationError,
    ) -> bool {
        matches!(
            &self.status,
            AccountStatus::Failed {
                error,
                saved_credential_state: SavedCredentialState::MayExist,
            } if error == &expected_error
        )
    }

    pub(crate) fn test_has_pending_permission_review(&self) -> bool {
        self.pending_permission_review.is_some()
    }

    pub(crate) fn test_pending_permission_review_is(
        &self,
        expected_request: Option<AccountPermissionRequest>,
    ) -> bool {
        self.pending_permission_review == expected_request
    }
}
