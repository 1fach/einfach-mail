//! Selects the saved account provider for application workflows.

use std::future::Future;

use einfach_mail_application::{
    AccountConnectionRequest, AccountOperationCancellation, AccountOperationError,
    AccountPermissionRequest, AccountProvider, CalendarDeltaRequest, CalendarDeltaResponse,
    CalendarOperationError, CalendarProvider, CalendarSyncStrategy, CalendarViewSnapshotResponse,
    ConnectedAccount, ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult, MailOperationError,
    MailProvider, MailSendProviderError, MessageDeltaRequest, MessageDeltaResponse,
    WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventUpdate, CalendarOccurrence,
    CalendarViewReference, EditableCalendarEvent, FolderReference, FollowUpFlagStatus, MailDraft,
    MailFolder, MailReplyDraft, MessageBody, MessageId, ReadState,
};
use einfach_mail_exchange_ews::ExchangeEwsProvider;
use einfach_mail_microsoft_graph::MicrosoftGraphProvider;
use tokio::sync::RwLock;

/// Combines Microsoft Graph and Exchange EWS behind the account provider ports.
#[derive(Clone)]
pub struct AccountProviderRouter<E> {
    graph: MicrosoftGraphProvider,
    ews: ExchangeEwsProvider<E>,
    microsoft365_available: bool,
    active_provider: std::sync::Arc<RwLock<Option<ProviderKind>>>,
}

impl<E> AccountProviderRouter<E> {
    /// Creates a router that restores and connects one provider-backed account.
    #[must_use]
    pub fn new(
        graph: MicrosoftGraphProvider,
        ews: ExchangeEwsProvider<E>,
        microsoft365_available: bool,
    ) -> Self {
        Self {
            graph,
            ews,
            microsoft365_available,
            active_provider: std::sync::Arc::new(RwLock::new(None)),
        }
    }

    async fn selected_provider(&self) -> Option<ProviderKind> {
        *self.active_provider.read().await
    }
}

#[derive(Clone, Copy)]
enum ProviderKind {
    MicrosoftGraph,
    ExchangeEws,
}

impl<E> AccountProvider for AccountProviderRouter<E>
where
    E: einfach_mail_application::ExchangeEwsAccountSettingsStore + 'static,
{
    async fn restore_account(&self) -> Result<Option<ConnectedAccount>, AccountOperationError> {
        if self.ews.saved_credential_account_id().await?.is_some() {
            *self.active_provider.write().await = Some(ProviderKind::ExchangeEws);
            return self.ews.restore_account().await;
        }

        *self.active_provider.write().await = Some(ProviderKind::MicrosoftGraph);
        let restored = self.graph.restore_account().await?;
        if restored.is_none() {
            *self.active_provider.write().await = None;
        }
        Ok(restored)
    }

    async fn saved_credential_account_id(
        &self,
    ) -> Result<Option<AccountId>, AccountOperationError> {
        if let Some(account_id) = self.ews.saved_credential_account_id().await? {
            return Ok(Some(account_id));
        }
        self.graph.saved_credential_account_id().await
    }

    async fn connect_account(
        &self,
        cancellation: AccountOperationCancellation,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        if self.selected_provider().await.is_some() {
            return Err(AccountOperationError::AccountAlreadyConnected);
        }
        if !self.microsoft365_available {
            return Err(AccountOperationError::ServiceUnavailable);
        }
        let profile = self.graph.connect_account(cancellation).await?;
        *self.active_provider.write().await = Some(ProviderKind::MicrosoftGraph);
        Ok(profile)
    }

    async fn connect_account_with_request(
        &self,
        request: AccountConnectionRequest,
        cancellation: AccountOperationCancellation,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        if self.selected_provider().await.is_some() {
            return Err(AccountOperationError::AccountAlreadyConnected);
        }
        let (kind, profile) = match request {
            AccountConnectionRequest::Microsoft365 => {
                if !self.microsoft365_available {
                    return Err(AccountOperationError::ServiceUnavailable);
                }
                (
                    ProviderKind::MicrosoftGraph,
                    self.graph.connect_account(cancellation).await?,
                )
            }
            request @ AccountConnectionRequest::ExchangeEws(_) => {
                if self.saved_credential_account_id().await?.is_some() {
                    return Err(AccountOperationError::AccountAlreadyConnected);
                }
                (
                    ProviderKind::ExchangeEws,
                    self.ews
                        .connect_account_with_request(request, cancellation)
                        .await?,
                )
            }
        };
        *self.active_provider.write().await = Some(kind);
        Ok(profile)
    }

    async fn discover_exchange_ews_endpoint(
        &self,
        request: ExchangeEwsDiscoveryRequest,
    ) -> Result<ExchangeEwsDiscoveryResult, AccountOperationError> {
        self.ews.discover_exchange_ews_endpoint(request).await
    }

    fn reauthorize_account(
        &self,
        expected_account_id: &AccountId,
        request: AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
        let expected_account_id = expected_account_id.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    if !self.microsoft365_available {
                        return Err(AccountOperationError::ServiceUnavailable);
                    }
                    self.graph
                        .reauthorize_account(&expected_account_id, request, cancellation)
                        .await
                }
                Some(ProviderKind::ExchangeEws) => {
                    self.ews
                        .reauthorize_account(&expected_account_id, request, cancellation)
                        .await
                }
                None => Err(AccountOperationError::AccountNotConnected),
            }
        }
    }

    async fn sign_out(&self) -> Result<(), AccountOperationError> {
        let provider = match self.selected_provider().await {
            Some(provider) => provider,
            None if self.ews.saved_credential_account_id().await?.is_some() => {
                ProviderKind::ExchangeEws
            }
            None => ProviderKind::MicrosoftGraph,
        };
        match provider {
            ProviderKind::MicrosoftGraph => self.graph.sign_out().await?,
            ProviderKind::ExchangeEws => self.ews.sign_out().await?,
        }
        *self.active_provider.write().await = None;
        Ok(())
    }
}

impl<E> MailProvider for AccountProviderRouter<E>
where
    E: einfach_mail_application::ExchangeEwsAccountSettingsStore + 'static,
{
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let account_id = account_id.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => self.graph.list_folders(&account_id).await,
                Some(ProviderKind::ExchangeEws) => self.ews.list_folders(&account_id).await,
                None => Err(MailOperationError::SignInRequired),
            }
        }
    }

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        let folder = folder.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph.message_delta_page(&folder, request).await
                }
                Some(ProviderKind::ExchangeEws) => {
                    self.ews.message_delta_page(&folder, request).await
                }
                None => Err(MailOperationError::SignInRequired),
            }
        }
    }

    async fn fetch_message_body<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
    ) -> Result<MessageBody, MailOperationError> {
        match self.selected_provider().await {
            Some(ProviderKind::MicrosoftGraph) => {
                self.graph.fetch_message_body(folder, message_id).await
            }
            Some(ProviderKind::ExchangeEws) => {
                self.ews.fetch_message_body(folder, message_id).await
            }
            None => Err(MailOperationError::SignInRequired),
        }
    }

    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        let folder = folder.clone();
        let message_id = message_id.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph
                        .set_message_read_state(&folder, &message_id, requested)
                        .await
                }
                Some(ProviderKind::ExchangeEws) => {
                    self.ews
                        .set_message_read_state(&folder, &message_id, requested)
                        .await
                }
                None => Err(MailOperationError::SignInRequired),
            }
        }
    }

    fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
        let folder = folder.clone();
        let message_id = message_id.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph
                        .set_message_follow_up_flag_status(&folder, &message_id, status)
                        .await
                }
                Some(ProviderKind::ExchangeEws) => {
                    self.ews
                        .set_message_follow_up_flag_status(&folder, &message_id, status)
                        .await
                }
                None => Err(MailOperationError::SignInRequired),
            }
        }
    }

    fn move_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        let source_folder = source_folder.clone();
        let message_id = message_id.clone();
        let destination_folder = destination_folder.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph
                        .move_message(&source_folder, &message_id, &destination_folder)
                        .await
                }
                Some(ProviderKind::ExchangeEws) => {
                    self.ews
                        .move_message(&source_folder, &message_id, &destination_folder)
                        .await
                }
                None => Err(MailOperationError::SignInRequired),
            }
        }
    }

    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        let account_id = account_id.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph.well_known_folder(&account_id, folder).await
                }
                Some(ProviderKind::ExchangeEws) => {
                    self.ews.well_known_folder(&account_id, folder).await
                }
                None => Err(MailOperationError::SignInRequired),
            }
        }
    }

    fn send_mail(
        &self,
        account_id: &AccountId,
        draft: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        let account_id = account_id.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph.send_mail(&account_id, draft).await
                }
                Some(ProviderKind::ExchangeEws) => self.ews.send_mail(&account_id, draft).await,
                None => Err(MailSendProviderError::SignInRequired),
            }
        }
    }

    fn reply_to_message(
        &self,
        account_id: &AccountId,
        draft: MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        let account_id = account_id.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph.reply_to_message(&account_id, draft).await
                }
                Some(ProviderKind::ExchangeEws) => {
                    self.ews.reply_to_message(&account_id, draft).await
                }
                None => Err(MailSendProviderError::SignInRequired),
            }
        }
    }
}

impl<E> CalendarProvider for AccountProviderRouter<E>
where
    E: einfach_mail_application::ExchangeEwsAccountSettingsStore + 'static,
{
    async fn calendar_sync_strategy(&self) -> Result<CalendarSyncStrategy, CalendarOperationError> {
        match self.selected_provider().await {
            Some(ProviderKind::MicrosoftGraph) => self.graph.calendar_sync_strategy().await,
            Some(ProviderKind::ExchangeEws) => self.ews.calendar_sync_strategy().await,
            None => Err(CalendarOperationError::SignInRequired),
        }
    }

    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        let view = view.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph.calendar_delta_page(&view, request).await
                }
                Some(ProviderKind::ExchangeEws) => {
                    self.ews.calendar_delta_page(&view, request).await
                }
                None => Err(CalendarOperationError::SignInRequired),
            }
        }
    }

    fn calendar_view_snapshot(
        &self,
        view: &CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarViewSnapshotResponse, CalendarOperationError>> + Send + '_
    {
        let view = view.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => {
                    self.graph.calendar_view_snapshot(&view).await
                }
                Some(ProviderKind::ExchangeEws) => self.ews.calendar_view_snapshot(&view).await,
                None => Err(CalendarOperationError::SignInRequired),
            }
        }
    }

    fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let creation = creation.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => self.graph.create_event(&creation).await,
                Some(ProviderKind::ExchangeEws) => self.ews.create_event(&creation).await,
                None => Err(CalendarOperationError::SignInRequired),
            }
        }
    }

    fn editable_event(
        &self,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<EditableCalendarEvent, CalendarOperationError>> + Send + '_
    {
        let event = event.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => self.graph.editable_event(&event).await,
                Some(ProviderKind::ExchangeEws) => self.ews.editable_event(&event).await,
                None => Err(CalendarOperationError::SignInRequired),
            }
        }
    }

    fn update_event(
        &self,
        update: &CalendarEventUpdate,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let update = update.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => self.graph.update_event(&update).await,
                Some(ProviderKind::ExchangeEws) => self.ews.update_event(&update).await,
                None => Err(CalendarOperationError::SignInRequired),
            }
        }
    }

    fn delete_event(
        &self,
        event: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        let event = event.clone();
        async move {
            match self.selected_provider().await {
                Some(ProviderKind::MicrosoftGraph) => self.graph.delete_event(&event).await,
                Some(ProviderKind::ExchangeEws) => self.ews.delete_event(&event).await,
                None => Err(CalendarOperationError::SignInRequired),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use einfach_mail_application::{
        AccountConnectionRequest, AccountOperationCancellation, AccountOperationError,
        AccountProvider, CalendarProvider, CalendarSyncStrategy, ExchangeEwsAccountSettings,
        ExchangeEwsAccountSettingsStore, ExchangeEwsSettingsError,
    };
    use einfach_mail_exchange_ews::ExchangeEwsProvider;
    use einfach_mail_microsoft_graph::MicrosoftGraphProvider;

    use super::{AccountProviderRouter, ProviderKind};

    #[derive(Clone)]
    struct EmptySettingsStore;

    impl ExchangeEwsAccountSettingsStore for EmptySettingsStore {
        async fn load(
            &self,
        ) -> Result<Option<ExchangeEwsAccountSettings>, ExchangeEwsSettingsError> {
            Ok(None)
        }

        async fn save(
            &self,
            _settings: ExchangeEwsAccountSettings,
        ) -> Result<(), ExchangeEwsSettingsError> {
            Ok(())
        }

        async fn delete(&self) -> Result<(), ExchangeEwsSettingsError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn refuses_to_replace_an_active_ews_account_with_graph() {
        let router = AccountProviderRouter::new(
            MicrosoftGraphProvider::new_unconfigured(),
            ExchangeEwsProvider::new(EmptySettingsStore),
            false,
        );
        *router.active_provider.write().await = Some(ProviderKind::ExchangeEws);

        let request_result = router
            .connect_account_with_request(
                AccountConnectionRequest::Microsoft365,
                AccountOperationCancellation::new(),
            )
            .await;
        let legacy_result = router
            .connect_account(AccountOperationCancellation::new())
            .await;

        assert_eq!(
            request_result,
            Err(AccountOperationError::AccountAlreadyConnected)
        );
        assert_eq!(
            legacy_result,
            Err(AccountOperationError::AccountAlreadyConnected)
        );
    }

    #[tokio::test]
    async fn reports_the_selected_provider_calendar_strategy() {
        let router = AccountProviderRouter::new(
            MicrosoftGraphProvider::new_unconfigured(),
            ExchangeEwsProvider::new(EmptySettingsStore),
            false,
        );
        assert_eq!(
            router.calendar_sync_strategy().await,
            Err(einfach_mail_application::CalendarOperationError::SignInRequired)
        );

        *router.active_provider.write().await = Some(ProviderKind::MicrosoftGraph);
        assert_eq!(
            router.calendar_sync_strategy().await,
            Ok(CalendarSyncStrategy::Delta)
        );

        *router.active_provider.write().await = Some(ProviderKind::ExchangeEws);
        assert_eq!(
            router.calendar_sync_strategy().await,
            Ok(CalendarSyncStrategy::Snapshot)
        );
    }
}
