use std::{collections::HashSet, future::Future, sync::Arc, time::SystemTime};

use einfach_mail_application::{
    AccountConnectionRequest, AccountOperationCancellation, AccountOperationError,
    AccountPermissionRequest, AccountProvider, CalendarDeltaRequest, CalendarDeltaResponse,
    CalendarOperationError, CalendarProvider, CalendarSyncStrategy, CalendarViewSnapshotResponse,
    ExchangeEwsAccountSettings, ExchangeEwsAccountSettingsStore, ExchangeEwsConnectionInput,
    ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult, MailOperationError, MailProvider,
    MailSendProviderError, MessageDeltaPageResponse, MessageDeltaRequest, MessageDeltaResponse,
    WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountCapabilities, AccountId, AccountProfile, FolderId, FolderReference, MailFolder, Mailbox,
    MessageBody, MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage,
    MessageDeltaProgress, MessageId, MessageMetadataPatch, ReadState,
};
use ews::{
    BaseFolderId, BaseItemId, BaseShape, Folder, FolderShape, ItemShape, PathToElement,
    ResponseClass,
    get_item::GetItem,
    response::ResponseCode,
    sync_folder_hierarchy::{Change as FolderChange, SyncFolderHierarchy},
    sync_folder_items::{Change as ItemChange, SyncFolderItems, SyncScope},
};
use keyring::{Entry, Error as KeyringError};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    autodiscover::EwsAutodiscover,
    calendar::{build_calendar_view_request, parse_calendar_view_snapshot},
    error::EwsError,
    transport::EwsTransport,
};
use einfach_mail_provider_runtime as worker;

const CREDENTIAL_SERVICE: &str = "com.1fach.einfach-mail";
const CREDENTIAL_ACCOUNT: &str = "exchange-ews-password";
const MAX_CREDENTIAL_BYTES: usize = 4096;
const MAX_SYNC_STATE_BYTES: usize = 128 * 1024;
const MAX_CURSOR_BYTES: usize = 256 * 1024;
const MAX_HIERARCHY_PAGES: usize = 100;
const MAX_FOLDERS: usize = 10_000;
const MAX_HIERARCHY_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_MESSAGE_BODY_BYTES: usize = 2 * 1024 * 1024;
const EWS_PAGE_SIZE: u16 = 512;

/// Connects to one on-premises Exchange account through Linux EWS and NTLMv2.
#[derive(Clone)]
pub struct ExchangeEwsProvider<S> {
    settings_store: Arc<S>,
    autodiscover: Arc<EwsAutodiscover>,
    session: Arc<RwLock<Option<Arc<EwsSession>>>>,
}

impl<S> ExchangeEwsProvider<S>
where
    S: ExchangeEwsAccountSettingsStore + 'static,
{
    /// Creates an EWS provider using the application's non-secret settings store.
    #[must_use]
    pub fn new(settings_store: S) -> Self {
        Self {
            settings_store: Arc::new(settings_store),
            autodiscover: Arc::new(EwsAutodiscover::default()),
            session: Arc::new(RwLock::new(None)),
        }
    }

    async fn connect_ews_account(
        settings_store: Arc<S>,
        session_store: Arc<RwLock<Option<Arc<EwsSession>>>>,
        input: ExchangeEwsConnectionInput,
        cancellation: AccountOperationCancellation,
    ) -> Result<AccountProfile, AccountOperationError> {
        let settings = validate_settings(&input)?;
        let password = SecretString::from(input.expose_password().to_owned());
        let credential_bytes = Zeroizing::new(input.expose_password().as_bytes().to_vec());
        if credential_bytes.is_empty() || credential_bytes.len() > MAX_CREDENTIAL_BYTES {
            return Err(AccountOperationError::InvalidConnectionDetails);
        }

        let session = Arc::new(EwsSession::new(settings.clone(), password)?);
        tokio::select! {
            () = cancellation.cancelled() => return Err(AccountOperationError::SignInCancelled),
            result = collect_folders(&session) => { result.map_err(AccountOperationError::from)?; }
        }
        cancellation.begin_credential_commit()?;

        blocking::unblock(move || save_password(&credential_bytes))
            .await
            .map_err(AccountOperationError::from)?;
        if settings_store.save(settings.clone()).await.is_err() {
            blocking::unblock(delete_password)
                .await
                .map_err(AccountOperationError::from)?;
            return Err(AccountOperationError::AccountStateUnavailable);
        }

        let profile = session.profile.clone();
        *session_store.write().await = Some(session);
        Ok(profile)
    }
}

impl<S> AccountProvider for ExchangeEwsProvider<S>
where
    S: ExchangeEwsAccountSettingsStore + 'static,
{
    fn restore_account(
        &self,
    ) -> impl Future<Output = Result<Option<AccountProfile>, AccountOperationError>> + Send + '_
    {
        let settings_store = Arc::clone(&self.settings_store);
        let session_store = Arc::clone(&self.session);
        worker::run(move |_cancellation: CancellationToken| async move {
            let Some(settings) = settings_store.load().await? else {
                blocking::unblock(delete_password)
                    .await
                    .map_err(AccountOperationError::from)?;
                *session_store.write().await = None;
                return Ok(None);
            };
            let password = blocking::unblock(load_password).await?;
            let Some(password) = password else {
                *session_store.write().await = None;
                return Ok(None);
            };
            let session = Arc::new(EwsSession::new(settings, password)?);
            collect_folders(&session)
                .await
                .map_err(AccountOperationError::from)?;
            let profile = session.profile.clone();
            *session_store.write().await = Some(session);
            Ok(Some(profile))
        })
    }

    fn saved_credential_account_id(
        &self,
    ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_ {
        let settings_store = Arc::clone(&self.settings_store);
        worker::run(move |_cancellation: CancellationToken| async move {
            let Some(settings) = settings_store.load().await? else {
                blocking::unblock(delete_password)
                    .await
                    .map_err(AccountOperationError::from)?;
                return Ok(None);
            };
            let password = blocking::unblock(load_password).await?;
            if password.is_none() {
                return Ok(None);
            }
            Ok(Some(profile_for(&settings)?.id().clone()))
        })
    }

    fn connect_account(
        &self,
        _cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        std::future::ready(Err(AccountOperationError::UnsupportedOperation))
    }

    fn connect_account_with_request(
        &self,
        request: AccountConnectionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        let settings_store = Arc::clone(&self.settings_store);
        let session_store = Arc::clone(&self.session);
        let commit_cancellation = cancellation.clone();
        async move {
            match request {
                AccountConnectionRequest::ExchangeEws(input) => {
                    worker::run_with_account_cancellation(
                        move |_worker_cancellation: CancellationToken| async move {
                            Self::connect_ews_account(
                                settings_store,
                                session_store,
                                input,
                                commit_cancellation,
                            )
                            .await
                        },
                        cancellation,
                    )
                    .await
                }
                AccountConnectionRequest::Microsoft365 => {
                    Err(AccountOperationError::UnsupportedOperation)
                }
            }
        }
    }

    fn discover_exchange_ews_endpoint(
        &self,
        request: ExchangeEwsDiscoveryRequest,
    ) -> impl Future<Output = Result<ExchangeEwsDiscoveryResult, AccountOperationError>> + Send + '_
    {
        let autodiscover = Arc::clone(&self.autodiscover);
        worker::run(move |_cancellation: CancellationToken| async move {
            autodiscover
                .discover(request)
                .await
                .map_err(AccountOperationError::from)
        })
    }

    fn reauthorize_account(
        &self,
        _expected_account_id: &AccountId,
        _request: AccountPermissionRequest,
        _cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        std::future::ready(Err(AccountOperationError::UnsupportedOperation))
    }

    fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
        let settings_store = Arc::clone(&self.settings_store);
        let session_store = Arc::clone(&self.session);
        worker::run(move |_cancellation: CancellationToken| async move {
            let previous = settings_store.load().await?;
            settings_store.delete().await?;
            if let Err(error) = blocking::unblock(delete_password).await {
                if let Some(settings) = previous {
                    settings_store
                        .save(settings)
                        .await
                        .map_err(AccountOperationError::from)?;
                }
                return Err(AccountOperationError::from(error));
            }
            *session_store.write().await = None;
            Ok(())
        })
    }
}

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
            let session = session_for_mail(&session_store, account_id).await?;
            worker::run(move |_cancellation: CancellationToken| async move {
                collect_folders(&session)
                    .await
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
            let session = session_for_mail(&session_store, folder.account_id().clone()).await?;
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
                let (messages, response_bytes) = session
                    .transport
                    .execute_with_size(operation)
                    .await
                    .map_err(MailOperationError::from)?;
                map_sync_responses(messages, folder.folder_id().as_str(), response_bytes)
            })
            .await
        }
    }

    fn fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
        let folder = folder.clone();
        let message_id = message_id.clone();
        let session_store = Arc::clone(&self.session);
        async move {
            let session = session_for_mail(&session_store, folder.account_id().clone()).await?;
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
                let mut responses = session
                    .transport
                    .execute(request)
                    .await
                    .map_err(MailOperationError::from)?;
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
    }

    fn set_message_read_state(
        &self,
        _folder: &FolderReference,
        _message_id: &MessageId,
        _requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        std::future::ready(Err(MailOperationError::UnsupportedOperation))
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

impl<S> CalendarProvider for ExchangeEwsProvider<S>
where
    S: ExchangeEwsAccountSettingsStore + 'static,
{
    fn calendar_sync_strategy(
        &self,
    ) -> impl Future<Output = Result<CalendarSyncStrategy, CalendarOperationError>> + Send + '_
    {
        std::future::ready(Ok(CalendarSyncStrategy::Snapshot))
    }

    fn calendar_delta_page(
        &self,
        _view: &einfach_mail_domain::CalendarViewReference,
        _request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn calendar_view_snapshot(
        &self,
        view: &einfach_mail_domain::CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarViewSnapshotResponse, CalendarOperationError>> + Send + '_
    {
        let view = view.clone();
        let session_store = Arc::clone(&self.session);
        async move {
            let session = session_for_calendar(&session_store, view.account_id()).await?;
            worker::run(move |_cancellation: CancellationToken| async move {
                let request =
                    build_calendar_view_request(&view).map_err(CalendarOperationError::from)?;
                let (document, response_bytes) = session
                    .transport
                    .execute_xml_with_size(request)
                    .await
                    .map_err(CalendarOperationError::from)?;
                parse_calendar_view_snapshot(&document, response_bytes, &view)
                    .map_err(CalendarOperationError::from)
            })
            .await
        }
    }

    fn create_event(
        &self,
        _creation: &einfach_mail_domain::CalendarEventCreation,
    ) -> impl Future<
        Output = Result<einfach_mail_domain::CalendarOccurrence, CalendarOperationError>,
    > + Send
    + '_ {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn editable_event(
        &self,
        _event: &einfach_mail_domain::CalendarOccurrence,
    ) -> impl Future<
        Output = Result<einfach_mail_domain::EditableCalendarEvent, CalendarOperationError>,
    > + Send
    + '_ {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn update_event(
        &self,
        _update: &einfach_mail_domain::CalendarEventUpdate,
    ) -> impl Future<
        Output = Result<einfach_mail_domain::CalendarOccurrence, CalendarOperationError>,
    > + Send
    + '_ {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn delete_event(
        &self,
        _event: &einfach_mail_domain::EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }
}

struct EwsSession {
    profile: AccountProfile,
    transport: EwsTransport,
}

async fn session_for_mail(
    sessions: &RwLock<Option<Arc<EwsSession>>>,
    account_id: AccountId,
) -> Result<Arc<EwsSession>, MailOperationError> {
    let session = sessions
        .read()
        .await
        .clone()
        .ok_or(MailOperationError::SignInRequired)?;
    if session.profile.id() != &account_id {
        return Err(MailOperationError::AccountMismatch);
    }
    Ok(session)
}

async fn session_for_calendar(
    sessions: &RwLock<Option<Arc<EwsSession>>>,
    account_id: &AccountId,
) -> Result<Arc<EwsSession>, CalendarOperationError> {
    let session = sessions
        .read()
        .await
        .clone()
        .ok_or(CalendarOperationError::SignInRequired)?;
    if session.profile.id() != account_id {
        return Err(CalendarOperationError::AccountMismatch);
    }
    Ok(session)
}

impl EwsSession {
    fn new(settings: ExchangeEwsAccountSettings, password: SecretString) -> Result<Self, EwsError> {
        let profile = profile_for(&settings).map_err(|_| EwsError::InvalidConnectionDetails)?;
        let transport = EwsTransport::new(
            settings.endpoint(),
            settings.username().to_owned(),
            password,
        )?;
        Ok(Self { profile, transport })
    }
}

fn validate_settings(
    input: &ExchangeEwsConnectionInput,
) -> Result<ExchangeEwsAccountSettings, AccountOperationError> {
    let endpoint = input.endpoint().trim();
    let mailbox = input.mailbox_address().trim();
    let username = input.username().trim();
    let password = input.expose_password();
    if endpoint.is_empty()
        || endpoint.len() > 2 * 1024
        || mailbox.is_empty()
        || mailbox.len() > 512
        || username.is_empty()
        || username.len() > 512
        || password.is_empty()
        || password.len() > MAX_CREDENTIAL_BYTES
        || [endpoint, mailbox, username, password]
            .iter()
            .any(|value| value.chars().any(char::is_control))
    {
        return Err(AccountOperationError::InvalidConnectionDetails);
    }
    let settings = ExchangeEwsAccountSettings::new(
        endpoint.to_owned(),
        mailbox.to_owned(),
        username.to_owned(),
    );
    profile_for(&settings).map_err(|_| AccountOperationError::InvalidConnectionDetails)?;
    Ok(settings)
}

fn profile_for(settings: &ExchangeEwsAccountSettings) -> Result<AccountProfile, EwsError> {
    let parsed_endpoint =
        url::Url::parse(settings.endpoint()).map_err(|_| EwsError::InvalidConnectionDetails)?;
    if parsed_endpoint.scheme() != "https"
        || parsed_endpoint.host_str().is_none()
        || !parsed_endpoint.username().is_empty()
        || parsed_endpoint.password().is_some()
        || parsed_endpoint.query().is_some()
        || parsed_endpoint.fragment().is_some()
    {
        return Err(EwsError::InvalidConnectionDetails);
    }
    let mailbox_address = settings.mailbox_address().trim();
    let account_key = format!(
        "{}\n{}",
        parsed_endpoint.as_str(),
        mailbox_address.to_ascii_lowercase()
    );
    let id = AccountId::try_from(format!("ews:{}", hex::encode(Sha256::digest(account_key))))
        .map_err(|_| EwsError::InvalidConnectionDetails)?;
    AccountProfile::try_new_with_capabilities(
        id,
        Some(mailbox_address.to_owned()),
        Some(mailbox_address.to_owned()),
        Some(settings.username().to_owned()),
        AccountCapabilities::read_only_mail(),
    )
    .map_err(|_| EwsError::InvalidConnectionDetails)
}

async fn collect_folders(session: &EwsSession) -> Result<Vec<MailFolder>, EwsError> {
    let mut sync_state = None;
    let mut seen_states = HashSet::new();
    let mut folders = Vec::new();
    let mut total_bytes = 0usize;
    for _ in 0..MAX_HIERARCHY_PAGES {
        let request = SyncFolderHierarchy {
            folder_shape: FolderShape {
                base_shape: BaseShape::Default,
            },
            sync_folder_id: Some(BaseFolderId::new_distinguished("msgfolderroot")),
            sync_state: sync_state.clone(),
        };
        let (mut messages, response_bytes) = session.transport.execute_with_size(request).await?;
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
            return Ok(folders);
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

fn message_metadata_shape() -> ItemShape {
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

fn map_item_change(change: ItemChange) -> Result<Option<MessageDeltaChange>, MailOperationError> {
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

fn map_sync_responses(
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

fn map_sync_page(
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

fn encode_cursor(
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

fn decode_cursor(link: &str) -> Result<EwsSyncCursor, MailOperationError> {
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
struct EwsSyncCursor {
    version: u8,
    folder_id: String,
    sync_state: Option<String>,
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

fn keyring_entry() -> Result<Entry, EwsError> {
    Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
        .map_err(|_| EwsError::CredentialStoreUnavailable)
}

fn load_password() -> Result<Option<SecretString>, EwsError> {
    let mut secret = match keyring_entry()?.get_secret() {
        Ok(secret) => Zeroizing::new(secret),
        Err(KeyringError::NoEntry) => return Ok(None),
        Err(_) => return Err(EwsError::CredentialStoreUnavailable),
    };
    if secret.is_empty() || secret.len() > MAX_CREDENTIAL_BYTES {
        return Err(EwsError::CredentialStoreUnavailable);
    }
    let password_bytes = std::mem::take(&mut *secret);
    Ok(Some(decode_password(password_bytes)?))
}

fn decode_password(password_bytes: Vec<u8>) -> Result<SecretString, EwsError> {
    match String::from_utf8(password_bytes) {
        Ok(password) => Ok(SecretString::from(password)),
        Err(error) => {
            let mut password_bytes = error.into_bytes();
            password_bytes.zeroize();
            Err(EwsError::CredentialStoreUnavailable)
        }
    }
}

fn save_password(password: &[u8]) -> Result<(), EwsError> {
    if password.is_empty() || password.len() > MAX_CREDENTIAL_BYTES {
        return Err(EwsError::InvalidConnectionDetails);
    }
    keyring_entry()?
        .set_secret(password)
        .map_err(|_| EwsError::CredentialStoreUnavailable)
}

fn delete_password() -> Result<(), EwsError> {
    match keyring_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Err(EwsError::CredentialStoreUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use einfach_mail_application::{MailOperationError, MessageDeltaResponse};
    use einfach_mail_domain::{
        AccountId, FolderId, FolderReference, MessageDeltaChange, MessageDeltaProgress, ReadState,
    };
    use ews::sync_folder_items::{Change, SyncFolderItems};

    use crate::error::EwsError;
    use crate::transport::parse_soap_response;

    use super::{
        decode_cursor, decode_password, encode_cursor, map_item_change, map_sync_responses,
        message_metadata_shape,
    };

    const SYNC_FOLDER_ITEMS_PAGE: &[u8] = include_bytes!("fixtures/sync-folder-items-page.xml");
    const SYNC_STATE_EXPIRED: &[u8] = include_bytes!("fixtures/sync-state-expired.xml");

    #[test]
    fn rejects_invalid_utf8_password_bytes_from_the_credential_store() {
        assert!(matches!(
            decode_password(vec![0xff]),
            Err(EwsError::CredentialStoreUnavailable)
        ));
    }

    #[test]
    fn cursor_is_scoped_to_one_folder_and_rejects_unknown_versions() {
        let cursor = encode_cursor("folder-1", Some("server-state".to_owned()))
            .expect("synthetic server cursor is encoded");
        assert_eq!(
            decode_cursor(cursor.as_str())
                .expect("encoded cursor is valid")
                .folder_id,
            "folder-1"
        );
        assert_eq!(
            decode_cursor(r#"{"version":2,"folder_id":"folder-1","sync_state":null}"#),
            Err(MailOperationError::InvalidResponse)
        );
        assert_eq!(
            decode_cursor("x".repeat(300_000).as_str()),
            Err(MailOperationError::ResourceLimitExceeded)
        );
    }

    #[test]
    fn initial_cursor_has_no_sync_state_and_metadata_shape_omits_bodies() {
        let cursor = encode_cursor("folder-1", None).expect("initial cursor is encoded");
        assert_eq!(
            decode_cursor(cursor.as_str())
                .expect("cursor is valid")
                .sync_state,
            None
        );
        let shape = message_metadata_shape();
        assert!(matches!(shape.base_shape, ews::BaseShape::IdOnly));
        assert_eq!(shape.include_mime_content, Some(false));
        assert_eq!(shape.additional_properties.as_ref().map(Vec::len), Some(6));
    }

    #[test]
    fn maps_read_flag_changes_without_requesting_a_full_message() {
        let change = Change::ReadFlagChange {
            item_id: ews::ItemId {
                id: "message-1".to_owned(),
                change_key: None,
            },
            is_read: true,
        };
        let mapped = map_item_change(change)
            .expect("synthetic read flag change maps")
            .expect("read flag change is retained");
        assert!(matches!(mapped, MessageDeltaChange::Upsert { .. }));
    }

    #[test]
    fn maps_a_synthetic_message_page_and_preserves_its_continuation_cursor() {
        let responses = parse_soap_response::<SyncFolderItems>(SYNC_FOLDER_ITEMS_PAGE)
            .expect("synthetic EWS page fixture parses");
        let response =
            map_sync_responses(responses, "synthetic-folder", SYNC_FOLDER_ITEMS_PAGE.len())
                .expect("synthetic EWS page maps");
        let MessageDeltaResponse::Page(response) = response else {
            panic!("successful EWS page maps to a message page");
        };
        let (page, response_bytes) = response.into_parts();
        assert_eq!(response_bytes, SYNC_FOLDER_ITEMS_PAGE.len());
        let (mut changes, progress) = page.into_parts();
        let MessageDeltaProgress::Next(link) = progress else {
            panic!("fixture indicates another page remains");
        };
        let cursor = decode_cursor(link.as_str()).expect("continuation cursor decodes");
        assert_eq!(cursor.folder_id, "synthetic-folder");
        assert_eq!(cursor.sync_state.as_deref(), Some("synthetic-page-cursor"));

        let Some(MessageDeltaChange::Upsert { id, patch }) = changes.pop() else {
            panic!("fixture includes one message upsert");
        };
        assert_eq!(id.as_str(), "synthetic-message-id");
        let account_id = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let folder_id = FolderId::try_from("synthetic-folder".to_owned())
            .expect("synthetic folder ID is valid");
        let message = patch
            .into_complete_message(FolderReference::new(account_id, folder_id), id)
            .expect("fixture includes complete metadata");
        assert_eq!(message.metadata().subject(), Some("Synthetic EWS fixture"));
        assert_eq!(message.metadata().read_state(), ReadState::Unread);
        assert_eq!(
            message
                .metadata()
                .origin()
                .from_mailbox()
                .map(|mailbox| mailbox.address()),
            Some("ada@example.test")
        );
    }

    #[test]
    fn maps_a_synthetic_expired_cursor_response_to_a_reset() {
        let responses = parse_soap_response::<SyncFolderItems>(SYNC_STATE_EXPIRED)
            .expect("expired-state EWS fixture parses");
        let response = map_sync_responses(responses, "synthetic-folder", SYNC_STATE_EXPIRED.len())
            .expect("expired EWS cursor requests a replacement snapshot");
        let MessageDeltaResponse::Reset(link) = response else {
            panic!("expired EWS cursor maps to a reset link");
        };
        let cursor = decode_cursor(link.as_str()).expect("reset cursor decodes");
        assert_eq!(cursor.folder_id, "synthetic-folder");
        assert_eq!(cursor.sync_state, None);
    }
}
