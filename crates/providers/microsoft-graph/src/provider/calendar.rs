use std::{future::Future, sync::Arc};

use huskarl::token::AccessToken;
use tokio_util::sync::CancellationToken;

use crate::{error::GraphError, worker};
use einfach_mail_application::{
    CalendarDeltaRequest, CalendarDeltaResponse, CalendarOperationError, CalendarProvider,
};
use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventUpdate, CalendarOccurrence,
    CalendarViewReference, EditableCalendarEvent,
};

use super::{
    MicrosoftGraphProvider, ProviderConfig,
    session::{AccessTokenError, ensure_access_token, invalidate_rejected_access_token},
};

impl From<AccessTokenError> for CalendarOperationError {
    fn from(error: AccessTokenError) -> Self {
        match error {
            AccessTokenError::SignInRequired => Self::SignInRequired,
            AccessTokenError::AccountMismatch => Self::AccountMismatch,
            AccessTokenError::AccessDenied => Self::AccessDenied,
            AccessTokenError::CredentialStoreUnavailable | AccessTokenError::ServiceUnavailable => {
                Self::ServiceUnavailable
            }
        }
    }
}

impl CalendarProvider for MicrosoftGraphProvider {
    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        let config = Arc::clone(&self.config);
        let view = view.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = view.account_id().clone();
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_calendar_graph_error)?;
            let page = graph_api
                .calendar_delta_page(&view, request, Arc::clone(&token))
                .await;
            map_calendar_result(&config, &account_id, &token, page).await
        })
    }

    fn create_event(
        &self,
        creation: &CalendarEventCreation,
    ) -> impl Future<
        Output = Result<einfach_mail_domain::CalendarOccurrence, CalendarOperationError>,
    > + Send
    + '_ {
        let config = Arc::clone(&self.config);
        let creation = creation.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = creation.view().account_id().clone();
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_calendar_graph_error)?;
            let event = graph_api
                .create_calendar_event(&creation, Arc::clone(&token))
                .await;
            map_calendar_result(&config, &account_id, &token, event).await
        })
    }

    fn editable_event(
        &self,
        event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<EditableCalendarEvent, CalendarOperationError>> + Send + '_
    {
        let config = Arc::clone(&self.config);
        let event = event.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = event.reference().account_id().clone();
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_calendar_graph_error)?;
            let editable = graph_api
                .editable_calendar_event(&event, Arc::clone(&token))
                .await;
            map_calendar_result(&config, &account_id, &token, editable).await
        })
    }

    fn update_event(
        &self,
        update: &CalendarEventUpdate,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let update = update.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = update
                .source()
                .occurrence()
                .reference()
                .account_id()
                .clone();
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_calendar_graph_error)?;
            let event = graph_api
                .update_calendar_event(&update, Arc::clone(&token))
                .await;
            map_calendar_result(&config, &account_id, &token, event).await
        })
    }

    fn delete_event(
        &self,
        event: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let event = event.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = event.occurrence().reference().account_id().clone();
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_calendar_graph_error)?;
            let result = graph_api
                .delete_calendar_event(&event, Arc::clone(&token))
                .await;
            map_calendar_result(&config, &account_id, &token, result).await
        })
    }
}

async fn map_calendar_result<T>(
    config: &ProviderConfig,
    account_id: &AccountId,
    access_token: &Arc<AccessToken>,
    calendar_result: Result<T, GraphError>,
) -> Result<T, CalendarOperationError> {
    let calendar_result =
        invalidate_rejected_access_token(config, account_id, access_token, calendar_result).await;
    calendar_result.map_err(map_calendar_graph_error)
}

fn map_calendar_graph_error(error: GraphError) -> CalendarOperationError {
    match error {
        GraphError::AccessTokenRejected => CalendarOperationError::SignInRequired,
        GraphError::AccessDenied => CalendarOperationError::AccessDenied,
        GraphError::CalendarEventCreationRejected => CalendarOperationError::EventCreationRejected,
        GraphError::CalendarEventCreationOutcomeUnknown => {
            CalendarOperationError::CreationOutcomeUnknown
        }
        GraphError::CalendarEventNotEditable => CalendarOperationError::EventNotEditable,
        GraphError::CalendarEventChanged => CalendarOperationError::EventChanged,
        GraphError::CalendarEventUpdateOutcomeUnknown => {
            CalendarOperationError::UpdateOutcomeUnknown
        }
        GraphError::CalendarEventDeletionRejected => CalendarOperationError::EventDeletionRejected,
        GraphError::CalendarEventDeletionOutcomeUnknown => {
            CalendarOperationError::DeleteOutcomeUnknown
        }
        GraphError::RateLimited { retry_after } => {
            CalendarOperationError::RateLimited { retry_after }
        }
        GraphError::ResponseTooLarge | GraphError::PaginationLimitExceeded => {
            CalendarOperationError::ResourceLimitExceeded
        }
        GraphError::InvalidResponse | GraphError::UnsafeLink | GraphError::InvalidRecord => {
            CalendarOperationError::InvalidResponse
        }
        GraphError::WorkerStart
        | GraphError::MessageUpdateOutcomeUnknown
        | GraphError::WorkerClosed
        | GraphError::Transport
        | GraphError::SendRejected
        | GraphError::SendOutcomeUnknown
        | GraphError::MessageMoveOutcomeUnknown
        | GraphError::InvalidToken => CalendarOperationError::ServiceUnavailable,
    }
}
