use std::{future::Future, sync::Arc};

use huskarl::token::AccessToken;
use tokio_util::sync::CancellationToken;

use crate::{error::GraphError, worker};
use einfach_mail_application::{
    MailOperationError, MailProvider, MailSendProviderError, MessageDeltaRequest,
    MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, FolderReference, FollowUpFlagStatus, MailDraft, MailFolder, MailMessage,
    MailReplyDraft, MessageBody, MessageId, ReadState,
};

use super::{
    MicrosoftGraphProvider, ProviderConfig,
    session::{AccessTokenError, ensure_access_token, invalidate_rejected_access_token},
};

impl From<AccessTokenError> for MailOperationError {
    fn from(error: AccessTokenError) -> Self {
        match error {
            AccessTokenError::SignInRequired => Self::SignInRequired,
            AccessTokenError::AccountMismatch => Self::AccountMismatch,
            AccessTokenError::AccessDenied => Self::AccessDenied,
            AccessTokenError::CredentialStoreUnavailable => Self::CredentialStoreUnavailable,
            AccessTokenError::ServiceUnavailable => Self::ServiceUnavailable,
        }
    }
}

impl From<AccessTokenError> for MailSendProviderError {
    fn from(error: AccessTokenError) -> Self {
        match error {
            AccessTokenError::SignInRequired => Self::SignInRequired,
            AccessTokenError::AccountMismatch => Self::AccountMismatch,
            AccessTokenError::AccessDenied => Self::PermissionRequired,
            AccessTokenError::CredentialStoreUnavailable | AccessTokenError::ServiceUnavailable => {
                Self::ServiceUnavailable
            }
        }
    }
}

impl MailProvider for MicrosoftGraphProvider {
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let account_id = account_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let folders = graph_api
                .list_folders(&account_id, Arc::clone(&token))
                .await;
            map_mail_result(&config, &account_id, &token, folders).await
        })
    }

    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let account_id = account_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let folder_result = graph_api
                .well_known_folder(&account_id, folder, Arc::clone(&token))
                .await;
            map_mail_result(&config, &account_id, &token, folder_result).await
        })
    }

    fn list_messages(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let folder = folder.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = folder.account_id();
            let token = ensure_access_token(&config, account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let messages = graph_api.list_messages(&folder, Arc::clone(&token)).await;
            map_mail_result(&config, account_id, &token, messages).await
        })
    }

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let folder = folder.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = folder.account_id();
            let token = ensure_access_token(&config, account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let page = graph_api
                .message_delta_page(&folder, request, Arc::clone(&token))
                .await;
            map_mail_result(&config, account_id, &token, page).await
        })
    }

    fn fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let account_id = folder.account_id().clone();
        let message_id = message_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let body = graph_api
                .fetch_message_body(&message_id, Arc::clone(&token))
                .await;
            map_mail_result(&config, &account_id, &token, body).await
        })
    }

    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let folder = folder.clone();
        let message_id = message_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = folder.account_id();
            let token = ensure_access_token(&config, account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let result = graph_api
                .set_message_read_state(&message_id, requested, Arc::clone(&token))
                .await;
            map_mail_result(&config, account_id, &token, result).await
        })
    }

    fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let folder = folder.clone();
        let message_id = message_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = folder.account_id();
            let token = ensure_access_token(&config, account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let result = graph_api
                .set_message_follow_up_flag_status(&message_id, status, Arc::clone(&token))
                .await;
            map_mail_result(&config, account_id, &token, result).await
        })
    }

    fn move_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let source_folder = source_folder.clone();
        let message_id = message_id.clone();
        let destination_folder = destination_folder.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            if source_folder.account_id() != destination_folder.account_id() {
                return Err(MailOperationError::AccountMismatch);
            }
            let account_id = source_folder.account_id();
            let token = ensure_access_token(&config, account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let result = graph_api
                .move_message(&message_id, &destination_folder, Arc::clone(&token))
                .await;
            map_mail_result(&config, account_id, &token, result).await
        })
    }

    fn send_mail(
        &self,
        account_id: &AccountId,
        draft: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let account_id = account_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_send_graph_error)?;
            let result = graph_api.send_mail(&draft, Arc::clone(&token)).await;
            map_send_submission_result(&config, &account_id, &token, result).await
        })
    }

    fn reply_to_message(
        &self,
        account_id: &AccountId,
        draft: MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let account_id = account_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            if draft.source().account_id() != &account_id {
                return Err(MailSendProviderError::AccountMismatch);
            }
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_send_graph_error)?;
            let result = graph_api.reply_to_message(&draft, Arc::clone(&token)).await;
            map_send_submission_result(&config, &account_id, &token, result).await
        })
    }
}

async fn map_mail_result<T>(
    config: &ProviderConfig,
    account_id: &AccountId,
    access_token: &Arc<AccessToken>,
    mail_result: Result<T, GraphError>,
) -> Result<T, MailOperationError> {
    let mail_result =
        invalidate_rejected_access_token(config, account_id, access_token, mail_result).await;
    mail_result.map_err(map_graph_error)
}

async fn map_send_submission_result(
    config: &ProviderConfig,
    account_id: &AccountId,
    access_token: &Arc<AccessToken>,
    result: Result<(), GraphError>,
) -> Result<(), MailSendProviderError> {
    let result = invalidate_rejected_access_token(config, account_id, access_token, result).await;
    result.map_err(map_send_graph_error)
}

fn map_graph_error(error: GraphError) -> MailOperationError {
    match error {
        GraphError::AccessTokenRejected => MailOperationError::SignInRequired,
        GraphError::AccessDenied => MailOperationError::AccessDenied,
        GraphError::RateLimited { retry_after } => MailOperationError::RateLimited { retry_after },
        GraphError::MessageUpdateOutcomeUnknown => MailOperationError::MessageUpdateOutcomeUnknown,
        GraphError::ResponseTooLarge | GraphError::PaginationLimitExceeded => {
            MailOperationError::ResourceLimitExceeded
        }
        GraphError::InvalidResponse | GraphError::UnsafeLink | GraphError::InvalidRecord => {
            MailOperationError::InvalidResponse
        }
        GraphError::MessageMoveOutcomeUnknown => MailOperationError::MoveOutcomeUnknown,
        GraphError::WorkerStart
        | GraphError::WorkerClosed
        | GraphError::Transport
        | GraphError::SendRejected
        | GraphError::SendOutcomeUnknown
        | GraphError::CalendarEventCreationRejected
        | GraphError::CalendarEventCreationOutcomeUnknown
        | GraphError::CalendarEventUpdateOutcomeUnknown
        | GraphError::CalendarEventDeletionRejected
        | GraphError::CalendarEventDeletionOutcomeUnknown
        | GraphError::CalendarEventNotEditable
        | GraphError::CalendarEventChanged
        | GraphError::InvalidToken => MailOperationError::ServiceUnavailable,
    }
}

fn map_send_graph_error(error: GraphError) -> MailSendProviderError {
    match error {
        GraphError::AccessTokenRejected => MailSendProviderError::SignInRequired,
        GraphError::AccessDenied => MailSendProviderError::PermissionRequired,
        GraphError::RateLimited { retry_after } => {
            MailSendProviderError::RateLimited { retry_after }
        }
        GraphError::SendRejected => MailSendProviderError::Rejected,
        GraphError::SendOutcomeUnknown => MailSendProviderError::OutcomeUnknown,
        GraphError::WorkerStart
        | GraphError::WorkerClosed
        | GraphError::Transport
        | GraphError::InvalidToken
        | GraphError::InvalidResponse
        | GraphError::UnsafeLink
        | GraphError::InvalidRecord
        | GraphError::ResponseTooLarge
        | GraphError::PaginationLimitExceeded
        | GraphError::MessageUpdateOutcomeUnknown
        | GraphError::MessageMoveOutcomeUnknown
        | GraphError::CalendarEventCreationRejected
        | GraphError::CalendarEventCreationOutcomeUnknown
        | GraphError::CalendarEventUpdateOutcomeUnknown
        | GraphError::CalendarEventDeletionRejected
        | GraphError::CalendarEventDeletionOutcomeUnknown
        | GraphError::CalendarEventNotEditable
        | GraphError::CalendarEventChanged => MailSendProviderError::ServiceUnavailable,
    }
}
