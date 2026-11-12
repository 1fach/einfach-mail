use std::{collections::HashSet, future::Future, sync::Arc, time::SystemTime};

use einfach_mail_application::{
    ExchangeEwsAccountSettingsStore, MailOperationError, MailProvider, MailSendProviderError,
    MessageDeltaPageResponse, MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, FolderId, FolderReference, MailAction, MailFolder, Mailbox, MessageBody,
    MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress,
    MessageId, MessageMetadataPatch, ReadState,
};
use einfach_mail_provider_runtime as worker;
use ews::{
    BaseFolderId, BaseItemId, BaseShape, Folder, FolderShape, ItemShape, PathToElement,
    ResponseClass,
    get_item::GetItem,
    response::ResponseCode,
    sync_folder_hierarchy::{Change as FolderChange, SyncFolderHierarchy},
    sync_folder_items::{Change as ItemChange, SyncFolderItems, SyncScope},
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{
    ExchangeEwsProvider,
    session::{EwsSession, session_for_mail},
};
use crate::{
    error::EwsError,
    provider::read_state::{
        CurrentEwsItemVersion, GetItemError, UpdateConfirmationError, confirm_update,
        current_item_version, get_item_request, parse_get_item_response,
        parse_update_item_response, serialize_update_item_request,
    },
    transport::{EwsRequestSequence, EwsWriteError},
    wire::ExchangeProductVersion,
};

const MAX_SYNC_STATE_BYTES: usize = 128 * 1024;
const MAX_CURSOR_BYTES: usize = 256 * 1024;
const MAX_HIERARCHY_PAGES: usize = 100;
const MAX_FOLDERS: usize = 10_000;
const MAX_HIERARCHY_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_MESSAGE_BODY_BYTES: usize = 2 * 1024 * 1024;
const EWS_PAGE_SIZE: u16 = 512;

pub(crate) mod read_state;

impl<S> MailProvider for ExchangeEwsProvider<S>
where
    S: ExchangeEwsAccountSettingsStore + 'static,
{
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let account_id = account_id.clone();
        let session_store = Arc::clone(&self.session);
        async move {
            let session = session_for_mail(&session_store, &account_id).await?;
            worker::run(move |_cancellation: CancellationToken| async move {
                collect_folders(&session)
                    .await
                    .map(|collection| collection.folders)
                    .map_err(MailOperationError::from)
            })
            .await
        }
    }

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        let folder = folder.clone();
        let session_store = Arc::clone(&self.session);
        async move {
            let session = session_for_mail(&session_store, folder.account_id()).await?;
            worker::run(move |_cancellation: CancellationToken| async move {
                let request_state = match request {
                    MessageDeltaRequest::Initial => None,
                    MessageDeltaRequest::Continue(link) => {
                        let cursor = decode_cursor(link.as_str())?;
                        if cursor.folder_id != folder.folder_id().as_str() {
                            return Err(MailOperationError::InvalidResponse);
                        }
                        cursor.sync_state
                    }
                };
                let operation = SyncFolderItems {
                    item_shape: message_metadata_shape(),
                    sync_folder_id: BaseFolderId::new_folder(folder.folder_id().as_str()),
                    sync_state: request_state,
                    ignore: None,
                    max_changes_returned: EWS_PAGE_SIZE,
                    sync_scope: Some(SyncScope::NormalItems),
                };
                let (response, response_bytes) = session
                    .transport
                    .execute_with_size(operation)
                    .await
                    .map_err(MailOperationError::from)?;
                map_sync_responses(
                    response.messages,
                    folder.folder_id().as_str(),
                    response_bytes,
                )
            })
            .await
        }
    }

    async fn fetch_message_body<'a>(
        &'a self,
        folder: &'a FolderReference,
        message_id: &'a MessageId,
    ) -> Result<MessageBody, MailOperationError> {
        let session = session_for_mail(&self.session, folder.account_id()).await?;
        let message_id = message_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let request = GetItem {
                item_shape: ItemShape {
                    base_shape: BaseShape::Default,
                    include_mime_content: Some(false),
                    additional_properties: None,
                },
                item_ids: vec![BaseItemId::ItemId {
                    id: message_id.as_str().to_owned(),
                    change_key: None,
                }],
            };
            let response = session
                .transport
                .execute(request)
                .await
                .map_err(MailOperationError::from)?;
            let mut responses = response.messages;
            if responses.len() != 1 {
                return Err(MailOperationError::InvalidResponse);
            }
            let response = match responses.pop().ok_or(MailOperationError::InvalidResponse)? {
                ResponseClass::Success(response) | ResponseClass::Warning(response) => response,
                ResponseClass::Error(error) => {
                    return Err(map_response_code(error.response_code).into());
                }
            };
            let item = response
                .items
                .inner
                .into_iter()
                .next()
                .ok_or(MailOperationError::InvalidResponse)?
                .into_inner_message();
            let body = item.body.ok_or(MailOperationError::InvalidResponse)?;
            let content = body.content.ok_or(MailOperationError::InvalidResponse)?;
            if content.len() > MAX_MESSAGE_BODY_BYTES {
                return Err(MailOperationError::ResourceLimitExceeded);
            }
            let body_type = match body.body_type {
                ews::BodyType::HTML => MessageBodyType::Html,
                ews::BodyType::Text => MessageBodyType::Text,
            };
            Ok(MessageBody::new(body_type, content))
        })
        .await
    }

    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        let folder = folder.clone();
        let message_id = message_id.clone();
        let session_store = Arc::clone(&self.session);
        async move {
            let session = session_for_mail(&session_store, folder.account_id()).await?;
            if !session
                .profile
                .capabilities()
                .mail()
                .supports(MailAction::SetReadState)
            {
                return Err(MailOperationError::UnsupportedOperation);
            }

            worker::run(move |_cancellation: CancellationToken| async move {
                update_message_read_state(&session, &message_id, requested).await
            })
            .await
        }
    }

    fn set_message_follow_up_flag_status(
        &self,
        _folder: &FolderReference,
        _message_id: &MessageId,
        _status: einfach_mail_domain::FollowUpFlagStatus,
    ) -> impl Future<Output = Result<einfach_mail_domain::FollowUpFlagStatus, MailOperationError>>
    + Send
    + '_ {
        std::future::ready(Err(MailOperationError::UnsupportedOperation))
    }

    fn move_message(
        &self,
        _source_folder: &FolderReference,
        _message_id: &MessageId,
        _destination_folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        std::future::ready(Err(MailOperationError::UnsupportedOperation))
    }

    fn well_known_folder(
        &self,
        _account_id: &AccountId,
        _folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        std::future::ready(Err(MailOperationError::UnsupportedOperation))
    }

    fn send_mail(
        &self,
        _account_id: &AccountId,
        _draft: einfach_mail_domain::MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        std::future::ready(Err(MailSendProviderError::UnsupportedOperation))
    }

    fn reply_to_message(
        &self,
        _account_id: &AccountId,
        _draft: einfach_mail_domain::MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        std::future::ready(Err(MailSendProviderError::UnsupportedOperation))
    }
}

pub(super) async fn update_message_read_state(
    session: &EwsSession,
    message_id: &MessageId,
    requested: ReadState,
) -> Result<ReadState, MailOperationError> {
    let sequence = session.transport.sequence().await;
    let current = current_message_version_for_write(&sequence, message_id).await?;
    let request =
        serialize_update_item_request(&current, requested).map_err(MailOperationError::from)?;
    let response = match sequence.execute_update_item(&request).await {
        Ok(response) => response,
        Err(EwsWriteError::NotDispatched(error)) => {
            return Err(MailOperationError::from(error));
        }
        Err(EwsWriteError::OutcomeUnknown) => {
            return Err(MailOperationError::MessageUpdateOutcomeUnknown);
        }
    };
    let parsed_response = parse_update_item_response(&response)
        .map_err(|_| MailOperationError::MessageUpdateOutcomeUnknown)?;
    match confirm_update(parsed_response, message_id, requested) {
        Ok(state) => Ok(state),
        Err(UpdateConfirmationError::Rejected(code)) => Err(map_response_code(code).into()),
        Err(UpdateConfirmationError::Unknown) => {
            Err(MailOperationError::MessageUpdateOutcomeUnknown)
        }
    }
}

pub(super) async fn current_message_version_for_write(
    sequence: &EwsRequestSequence<'_>,
    message_id: &MessageId,
) -> Result<CurrentEwsItemVersion, MailOperationError> {
    let (document, _) = sequence
        .execute_xml_with_size(get_item_request(message_id))
        .await
        .map_err(MailOperationError::from)?;
    let parsed_response = parse_get_item_response(&document).map_err(MailOperationError::from)?;
    let (server_version_info, current) = match current_item_version(parsed_response, message_id) {
        Ok(current) => current,
        Err(GetItemError::Rejected(code)) => return Err(map_response_code(code).into()),
        Err(GetItemError::InvalidResponse) => return Err(MailOperationError::InvalidResponse),
    };
    let server_version = server_version_info
        .as_ref()
        .and_then(ExchangeProductVersion::from_server_version_info);
    if !server_version.is_some_and(ExchangeProductVersion::supports_read_receipt_suppression) {
        return Err(MailOperationError::UnsupportedOperation);
    }
    Ok(current)
}

pub(super) struct EwsFolderCollection {
    pub(super) folders: Vec<MailFolder>,
    pub(super) server_version: Option<ExchangeProductVersion>,
}

pub(super) async fn collect_folders(session: &EwsSession) -> Result<EwsFolderCollection, EwsError> {
    let mut sync_state = None;
    let mut seen_states = HashSet::new();
    let mut folders = Vec::new();
    let mut total_bytes = 0usize;
    let mut server_version = None;
    for _ in 0..MAX_HIERARCHY_PAGES {
        let request = SyncFolderHierarchy {
            folder_shape: FolderShape {
                base_shape: BaseShape::Default,
            },
            sync_folder_id: Some(BaseFolderId::new_distinguished("msgfolderroot")),
            sync_state: sync_state.clone(),
        };
        let (soap_response, response_bytes) = session.transport.execute_with_size(request).await?;
        if sync_state.is_none() {
            server_version = soap_response
                .server_version
                .as_ref()
                .and_then(ExchangeProductVersion::from_server_version_info);
        }
        let mut messages = soap_response.messages;
        total_bytes = total_bytes
            .checked_add(response_bytes)
            .ok_or(EwsError::ResourceLimitExceeded)?;
        if total_bytes > MAX_HIERARCHY_RESPONSE_BYTES || messages.len() != 1 {
            return Err(EwsError::ResourceLimitExceeded);
        }
        let response = match messages.pop().ok_or(EwsError::InvalidResponse)? {
            ResponseClass::Success(response) | ResponseClass::Warning(response) => response,
            ResponseClass::Error(error) => return Err(map_response_code(error.response_code)),
        };
        if response.sync_state.len() > MAX_SYNC_STATE_BYTES
            || !seen_states.insert(response.sync_state.clone())
        {
            return Err(EwsError::InvalidResponse);
        }
        for change in response.changes.inner {
            if let FolderChange::Create { folder } | FolderChange::Update { folder } = change
                && let Some(folder) = map_folder(session.profile.id(), folder)?
            {
                folders.push(folder);
                if folders.len() > MAX_FOLDERS {
                    return Err(EwsError::ResourceLimitExceeded);
                }
            }
        }
        if response.includes_last_folder_in_range {
            return Ok(EwsFolderCollection {
                folders,
                server_version,
            });
        }
        sync_state = Some(response.sync_state);
    }
    Err(EwsError::ResourceLimitExceeded)
}

fn map_folder(account_id: &AccountId, folder: Folder) -> Result<Option<MailFolder>, EwsError> {
    let (folder_id, parent_id, folder_class, display_name) = match folder {
        Folder::Folder {
            folder_id,
            parent_folder_id,
            folder_class,
            display_name,
            ..
        }
        | Folder::CalendarFolder {
            folder_id,
            parent_folder_id,
            folder_class,
            display_name,
            ..
        }
        | Folder::ContactsFolder {
            folder_id,
            parent_folder_id,
            folder_class,
            display_name,
            ..
        }
        | Folder::SearchFolder {
            folder_id,
            parent_folder_id,
            folder_class,
            display_name,
            ..
        }
        | Folder::TasksFolder {
            folder_id,
            parent_folder_id,
            folder_class,
            display_name,
            ..
        } => (folder_id, parent_folder_id, folder_class, display_name),
    };
    if !folder_class
        .as_deref()
        .is_some_and(|class| class == "IPF.Note" || class.starts_with("IPF.Note."))
    {
        return Ok(None);
    }
    let folder_id = folder_id.ok_or(EwsError::InvalidResponse)?.id;
    let reference = FolderReference::new(
        account_id.clone(),
        FolderId::try_from(folder_id).map_err(|_| EwsError::InvalidResponse)?,
    );
    let parent = parent_id
        .map(|parent| {
            FolderId::try_from(parent.id)
                .map(|parent_id| FolderReference::new(account_id.clone(), parent_id))
                .map_err(|_| EwsError::InvalidResponse)
        })
        .transpose()?;
    let folder = MailFolder::new(reference, parent, display_name.unwrap_or_default())
        .map_err(|_| EwsError::InvalidResponse)?;
    Ok(Some(folder))
}

pub(super) fn message_metadata_shape() -> ItemShape {
    ItemShape {
        base_shape: BaseShape::IdOnly,
        include_mime_content: Some(false),
        additional_properties: Some(
            [
                "item:ItemClass",
                "item:Subject",
                "item:DateTimeReceived",
                "message:From",
                "message:Sender",
                "message:IsRead",
            ]
            .into_iter()
            .map(|field_uri| PathToElement::FieldURI {
                field_URI: field_uri.to_owned(),
            })
            .collect(),
        ),
    }
}

pub(super) fn map_item_change(
    change: ItemChange,
) -> Result<Option<MessageDeltaChange>, MailOperationError> {
    match change {
        ItemChange::Delete { item_id } => Ok(Some(MessageDeltaChange::Removed {
            id: MessageId::try_from(item_id.id).map_err(|_| MailOperationError::InvalidResponse)?,
        })),
        ItemChange::ReadFlagChange { item_id, is_read } => {
            let id =
                MessageId::try_from(item_id.id).map_err(|_| MailOperationError::InvalidResponse)?;
            let mut patch = MessageMetadataPatch::new();
            patch.set_read_state(if is_read {
                ReadState::Read
            } else {
                ReadState::Unread
            });
            Ok(Some(MessageDeltaChange::Upsert { id, patch }))
        }
        ItemChange::Create { item } | ItemChange::Update { item } => {
            map_message(item).map_err(MailOperationError::from)
        }
    }
}

pub(super) fn map_sync_responses(
    mut responses: Vec<ews::ResponseClass<ews::sync_folder_items::SyncFolderItemsResponseMessage>>,
    folder_id: &str,
    response_bytes: usize,
) -> Result<MessageDeltaResponse, MailOperationError> {
    if responses.len() != 1 {
        return Err(MailOperationError::InvalidResponse);
    }
    match responses.pop().ok_or(MailOperationError::InvalidResponse)? {
        ResponseClass::Success(response) | ResponseClass::Warning(response) => {
            map_sync_page(response, folder_id, response_bytes)
        }
        ResponseClass::Error(error)
            if error.response_code == ResponseCode::ErrorInvalidSyncStateData =>
        {
            let link = encode_cursor(folder_id, None)?;
            Ok(MessageDeltaResponse::Reset(link))
        }
        ResponseClass::Error(error) => Err(map_response_code(error.response_code).into()),
    }
}

pub(super) fn map_sync_page(
    response: ews::sync_folder_items::SyncFolderItemsResponseMessage,
    folder_id: &str,
    response_bytes: usize,
) -> Result<MessageDeltaResponse, MailOperationError> {
    if response.sync_state.len() > MAX_SYNC_STATE_BYTES {
        return Err(MailOperationError::ResourceLimitExceeded);
    }
    let mut changes = Vec::with_capacity(response.changes.inner.len());
    for change in response.changes.inner {
        if let Some(change) = map_item_change(change)? {
            changes.push(change);
        }
    }
    let link = encode_cursor(folder_id, Some(response.sync_state))?;
    let progress = if response.includes_last_item_in_range {
        MessageDeltaProgress::Complete(link)
    } else {
        MessageDeltaProgress::Next(link)
    };
    let page = MessageDeltaPage::new(changes, progress);
    Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
        page,
        response_bytes,
    )))
}

fn map_message(item: ews::RealItem) -> Result<Option<MessageDeltaChange>, EwsError> {
    let message = item.into_inner_message();
    if !message.item_class.as_deref().is_some_and(|class| {
        class.starts_with("IPM.Note") || class.starts_with("IPM.Schedule.Meeting")
    }) {
        return Ok(None);
    }
    let item_id = message.item_id.ok_or(EwsError::InvalidResponse)?;
    let received_at = message
        .date_time_received
        .ok_or(EwsError::InvalidResponse)?
        .0;
    let is_read = message.is_read.ok_or(EwsError::InvalidResponse)?;
    let id = MessageId::try_from(item_id.id).map_err(|_| EwsError::InvalidResponse)?;
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(message.subject);
    patch.set_from(map_recipient(message.from));
    patch.set_sender(map_recipient(message.sender));
    patch.set_received_at(SystemTime::from(received_at));
    patch.set_read_state(if is_read {
        ReadState::Read
    } else {
        ReadState::Unread
    });
    Ok(Some(MessageDeltaChange::Upsert { id, patch }))
}

fn map_recipient(recipient: Option<ews::Recipient>) -> Option<Mailbox> {
    recipient.and_then(|recipient| {
        let address = recipient.mailbox.email_address?;
        Mailbox::try_new(address, recipient.mailbox.name).ok()
    })
}

pub(super) fn encode_cursor(
    folder_id: &str,
    sync_state: Option<String>,
) -> Result<MessageDeltaLink, MailOperationError> {
    if sync_state
        .as_ref()
        .is_some_and(|state| state.len() > MAX_SYNC_STATE_BYTES)
    {
        return Err(MailOperationError::ResourceLimitExceeded);
    }
    let encoded = serde_json::to_string(&EwsSyncCursor {
        version: 1,
        folder_id: folder_id.to_owned(),
        sync_state,
    })
    .map_err(|_| MailOperationError::InvalidResponse)?;
    if encoded.len() > MAX_CURSOR_BYTES {
        return Err(MailOperationError::ResourceLimitExceeded);
    }
    Ok(MessageDeltaLink::from_opaque(encoded))
}

pub(super) fn decode_cursor(link: &str) -> Result<EwsSyncCursor, MailOperationError> {
    if link.len() > MAX_CURSOR_BYTES {
        return Err(MailOperationError::ResourceLimitExceeded);
    }
    let cursor: EwsSyncCursor =
        serde_json::from_str(link).map_err(|_| MailOperationError::InvalidResponse)?;
    if cursor.version != 1
        || cursor.folder_id.is_empty()
        || cursor.folder_id.len() > 1024
        || cursor
            .sync_state
            .as_ref()
            .is_some_and(|state| state.len() > MAX_SYNC_STATE_BYTES)
    {
        return Err(MailOperationError::InvalidResponse);
    }
    Ok(cursor)
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EwsSyncCursor {
    pub(super) version: u8,
    pub(super) folder_id: String,
    pub(super) sync_state: Option<String>,
}

fn map_response_code(code: ResponseCode) -> EwsError {
    match code {
        ResponseCode::ErrorAccessDenied | ResponseCode::ErrorImpersonateUserDenied => {
            EwsError::AccessDenied
        }
        ResponseCode::ErrorInvalidSyncStateData => EwsError::SyncStateExpired,
        _ => EwsError::InvalidResponse,
    }
}
