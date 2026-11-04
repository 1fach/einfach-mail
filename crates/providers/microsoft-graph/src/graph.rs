//! Microsoft Graph profile, folder, and message mapping.

use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::SystemTime,
};

use huskarl::token::AccessToken;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};
use url::Url;

use crate::{
    error::GraphError,
    graph_url::{GraphAuthority, GraphUrl},
    transport::{GraphOperation, GraphResponse, GraphTransportHandle},
};
use einfach_mail_application::{
    CalendarDeltaPageResponse, CalendarDeltaRequest, MessageDeltaPageResponse, MessageDeltaRequest,
    MessageDeltaResponse,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage,
    CalendarDeltaProgress, CalendarEventCreation, CalendarEventUpdate, CalendarEventVersion,
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, CalendarViewReference, EditableCalendarEvent, EventId, FolderId,
    FolderReference, FollowUpFlagStatus, MailDraft, MailFolder, MailMessage, MailReplyDraft,
    Mailbox, MessageBody, MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage,
    MessageDeltaProgress, MessageId, MessageMetadata, MessageMetadataPatch, MessageOrigin,
    ReadState,
};

const MAX_PAGINATION_PAGES: usize = 1_000;
const MAX_PAGINATION_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct GraphApi {
    authority: GraphAuthority,
    transport: GraphTransportHandle,
}

impl GraphApi {
    pub(super) fn new(authority: GraphAuthority, transport: GraphTransportHandle) -> Self {
        Self {
            authority,
            transport,
        }
    }

    pub(super) async fn load_profile(
        &self,
        endpoint: &Url,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<AccountProfile, GraphError> {
        let profile_url = self.authority.profile_url(endpoint)?;
        let response = self
            .transport
            .get(GraphOperation::Profile, profile_url, token)
            .await?;
        let profile = serde_json::from_slice::<GraphUserProfile>(&response)
            .map_err(|_| GraphError::InvalidResponse)?;
        profile.try_into()
    }

    pub(super) async fn list_folders(
        &self,
        account_id: &AccountId,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<Vec<MailFolder>, GraphError> {
        let mut traversal = PageTraversal::new(self, GraphOperation::Folders, token);
        let roots = traversal
            .collect::<WireFolder, Option<MailFolder>>(
                self.authority.root_folders_path()?,
                |folder| folder.into_domain(account_id, None),
            )
            .await?
            .into_iter()
            .flatten()
            .collect();
        let mut tree = FolderTree::new();
        tree.add(roots)?;
        while let Some(parent_id) = tree.pending.pop_front() {
            let children = traversal
                .collect::<WireFolder, Option<MailFolder>>(
                    self.authority.child_folders_path(parent_id.as_str())?,
                    |folder| folder.into_domain(account_id, Some(parent_id.clone())),
                )
                .await?
                .into_iter()
                .flatten()
                .collect();
            tree.add(children)?;
        }
        Ok(tree.folders)
    }

    pub(super) async fn deleted_items_folder_id(
        &self,
        token: Arc<AccessToken>,
    ) -> Result<FolderId, GraphError> {
        let response = self
            .transport
            .get(
                GraphOperation::Folders,
                self.authority.deleted_items_folder_path()?,
                token,
            )
            .await?;
        let folder = serde_json::from_slice::<WireFolderId>(&response)
            .map_err(|_| GraphError::InvalidResponse)?;
        FolderId::try_from(folder.id).map_err(|_| GraphError::InvalidRecord)
    }

    pub(super) async fn list_messages(
        &self,
        folder: &FolderReference,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<Vec<MailMessage>, GraphError> {
        let mut traversal = PageTraversal::new(self, GraphOperation::Messages, token);
        let folder = folder.clone();
        traversal
            .collect::<WireMessage, MailMessage>(
                self.authority
                    .folder_messages_path(folder.folder_id().as_str())?,
                |message| message.into_domain(folder.clone()),
            )
            .await
    }

    pub(super) async fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<MessageDeltaResponse, GraphError> {
        let request_url = match request {
            MessageDeltaRequest::Initial => self
                .authority
                .folder_message_delta_path(folder.folder_id().as_str())?,
            MessageDeltaRequest::Continue(link) => self.authority.opaque_link(link.as_str())?,
        };
        let response = self.transport.get_message_delta(request_url, token).await?;
        let response = match response {
            GraphResponse::Body(body) => body,
            GraphResponse::MessageDeltaReset(Some(location)) => {
                let reset_link = self
                    .authority
                    .message_delta_reset_link(folder.folder_id().as_str(), &location)?;
                return Ok(MessageDeltaResponse::Reset(MessageDeltaLink::from_opaque(
                    reset_link.as_str().to_owned(),
                )));
            }
            GraphResponse::MessageDeltaReset(None) => return Err(GraphError::InvalidResponse),
        };
        let response_bytes = response.len();
        let page = serde_json::from_slice::<GraphDeltaPage<WireMessageChange>>(&response)
            .map_err(|_| GraphError::InvalidResponse)?;
        let progress = self.delta_progress(page.next_link, page.delta_link)?;
        let changes = page
            .records
            .into_iter()
            .map(MessageDeltaChange::try_from)
            .collect::<Result<_, _>>()?;
        Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
            MessageDeltaPage::new(changes, progress),
            response_bytes,
        )))
    }

    pub(super) async fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        request: CalendarDeltaRequest,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<CalendarDeltaPageResponse, GraphError> {
        let request_url = match request {
            CalendarDeltaRequest::Initial => {
                self.authority.calendar_view_delta_path(view.range())?
            }
            CalendarDeltaRequest::Continue(link) => self.authority.opaque_link(link.as_str())?,
        };
        let response = self
            .transport
            .get(GraphOperation::CalendarViewDelta, request_url, token)
            .await?;
        let response_bytes = response.len();
        let page = self.parse_calendar_delta_page(&response)?;
        Ok(CalendarDeltaPageResponse::new(page, response_bytes))
    }

    pub(super) async fn create_calendar_event(
        &self,
        creation: &CalendarEventCreation,
        token: Arc<AccessToken>,
    ) -> Result<CalendarOccurrence, GraphError> {
        let url = self.authority.primary_calendar_events_path()?;
        let body = create_calendar_event_body(creation)?;
        let response = self
            .transport
            .post_json(GraphOperation::CalendarEventCreate, url, token, body)
            .await?;
        let event = serde_json::from_slice::<WireCalendarEvent>(&response)
            .map_err(|_| GraphError::CalendarEventCreationOutcomeUnknown)?;
        event
            .into_new(creation.view().clone())
            .map_err(|_| GraphError::CalendarEventCreationOutcomeUnknown)
    }

    pub(super) async fn editable_calendar_event(
        &self,
        event: &CalendarOccurrence,
        token: Arc<AccessToken>,
    ) -> Result<EditableCalendarEvent, GraphError> {
        let editable = self
            .fetch_editable_calendar_event(event, token)
            .await?
            .editable;
        if editable.occurrence().reference() != event.reference()
            || editable.occurrence().id() != event.id()
        {
            return Err(GraphError::InvalidRecord);
        }
        Ok(editable)
    }

    async fn fetch_editable_calendar_event(
        &self,
        event: &CalendarOccurrence,
        token: Arc<AccessToken>,
    ) -> Result<CheckedEditableCalendarEvent, GraphError> {
        let view = event.reference().clone();
        let url = self
            .authority
            .editable_calendar_event_path(event.id().as_str())?;
        let response = self
            .transport
            .get(GraphOperation::CalendarEventRead, url, token)
            .await?;
        let event = serde_json::from_slice::<WireCalendarEvent>(&response)
            .map_err(|_| GraphError::InvalidResponse)?;
        event.into_checked_editable(view)
    }

    pub(super) async fn update_calendar_event(
        &self,
        update: &CalendarEventUpdate,
        token: Arc<AccessToken>,
    ) -> Result<CalendarOccurrence, GraphError> {
        let current = self
            .current_editable_calendar_event(update.source(), Arc::clone(&token))
            .await?;
        self.patch_calendar_event(update, &current.editable, token)
            .await
    }

    pub(super) async fn delete_calendar_event(
        &self,
        event: &EditableCalendarEvent,
        token: Arc<AccessToken>,
    ) -> Result<(), GraphError> {
        let current = self
            .current_editable_calendar_event(event, Arc::clone(&token))
            .await?;
        let etag = current.etag.as_deref().ok_or(GraphError::InvalidResponse)?;
        let url = self
            .authority
            .calendar_event_path(event.occurrence().id().as_str())?;
        self.transport.delete_calendar_event(url, token, etag).await
    }

    async fn current_editable_calendar_event(
        &self,
        source: &EditableCalendarEvent,
        token: Arc<AccessToken>,
    ) -> Result<CheckedEditableCalendarEvent, GraphError> {
        let source_event = source.occurrence();
        let current = self
            .fetch_editable_calendar_event(source_event, Arc::clone(&token))
            .await
            .map_err(|error| match error {
                GraphError::CalendarEventNotEditable => GraphError::CalendarEventChanged,
                other => other,
            })?;
        if current.editable.occurrence().reference() != source_event.reference()
            || current.editable.occurrence().id() != source_event.id()
            || current.editable.version() != source.version()
        {
            return Err(GraphError::CalendarEventChanged);
        }
        Ok(current)
    }

    async fn patch_calendar_event(
        &self,
        update: &CalendarEventUpdate,
        current: &EditableCalendarEvent,
        token: Arc<AccessToken>,
    ) -> Result<CalendarOccurrence, GraphError> {
        let body = calendar_event_update_body(update)?;
        let event_id = update.source().occurrence().id();
        let response = self
            .transport
            .patch_json(
                GraphOperation::CalendarEventUpdate,
                self.authority.calendar_event_path(event_id.as_str())?,
                token,
                body.into(),
            )
            .await?;
        confirm_calendar_event_update(update, current, &response)
    }

    fn parse_calendar_delta_page(&self, response: &[u8]) -> Result<CalendarDeltaPage, GraphError> {
        let page = serde_json::from_slice::<GraphDeltaPage<WireCalendarEvent>>(response)
            .map_err(|_| GraphError::InvalidResponse)?;
        let progress = self.calendar_delta_progress(page.next_link, page.delta_link)?;
        let changes = page
            .records
            .into_iter()
            .map(WireCalendarEvent::into_change)
            .collect::<Result<_, _>>()?;
        Ok(CalendarDeltaPage::new(changes, progress))
    }

    pub(super) async fn fetch_message_body(
        &self,
        message_id: &MessageId,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<MessageBody, GraphError> {
        let url = self.authority.message_body_path(message_id.as_str())?;
        let response = self
            .transport
            .get(GraphOperation::MessageBody, url, token)
            .await?;
        let message = serde_json::from_slice::<WireMessageBody>(&response)
            .map_err(|_| GraphError::InvalidResponse)?;
        let content_type = match message.body.content_type.as_str() {
            "html" => MessageBodyType::Html,
            "text" => MessageBodyType::Text,
            _ => return Err(GraphError::InvalidRecord),
        };
        Ok(MessageBody::new(content_type, message.body.content))
    }

    pub(super) async fn set_message_read_state(
        &self,
        message_id: &MessageId,
        requested: ReadState,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<ReadState, GraphError> {
        let response = self
            .transport
            .patch_json(
                GraphOperation::MessageUpdate,
                self.authority.message_update_path(message_id.as_str())?,
                token,
                read_state_patch_body(requested),
            )
            .await?;
        confirmed_read_state(&response, requested)
    }

    pub(super) async fn set_message_follow_up_flag_status(
        &self,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<FollowUpFlagStatus, GraphError> {
        let response = self
            .transport
            .patch_json(
                GraphOperation::MessageUpdate,
                self.authority.message_update_path(message_id.as_str())?,
                token,
                follow_up_flag_patch_body(status),
            )
            .await?;
        confirmed_follow_up_flag_status(&response, status)
    }

    pub(super) async fn move_message(
        &self,
        message_id: &MessageId,
        destination_folder: &FolderReference,
        token: Arc<AccessToken>,
    ) -> Result<(), GraphError> {
        let url = self.authority.message_move_path(message_id.as_str())?;
        let response = self
            .transport
            .post_json(
                GraphOperation::MessageMove,
                url,
                token,
                message_move_body(destination_folder.folder_id().as_str())?
                    .as_ref()
                    .to_vec(),
            )
            .await?;
        confirmed_message_move(
            &response,
            message_id,
            destination_folder.folder_id().as_str(),
        )
    }

    pub(super) async fn send_mail(
        &self,
        draft: &MailDraft,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<(), GraphError> {
        let body = serde_json::to_vec(&SendMailRequest::from(draft))
            .map_err(|_| GraphError::InvalidRecord)?;
        self.transport
            .post_mail(self.authority.mail_send_path()?, token, body)
            .await
    }

    pub(super) async fn reply_to_message(
        &self,
        draft: &MailReplyDraft,
        token: Arc<AccessToken>,
    ) -> Result<(), GraphError> {
        let url = self
            .authority
            .message_reply_path(draft.source().message_id().as_str())?;
        let body = mail_reply_body(draft)?;
        self.transport.post_mail(url, token, body).await
    }

    fn delta_progress(
        &self,
        next_link: Option<String>,
        delta_link: Option<String>,
    ) -> Result<MessageDeltaProgress, GraphError> {
        match (next_link, delta_link) {
            (Some(link), None) => Ok(MessageDeltaProgress::Next(MessageDeltaLink::from_opaque(
                self.authority.opaque_link(&link)?.as_str().to_owned(),
            ))),
            (None, Some(link)) => Ok(MessageDeltaProgress::Complete(
                MessageDeltaLink::from_opaque(
                    self.authority.opaque_link(&link)?.as_str().to_owned(),
                ),
            )),
            _ => Err(GraphError::InvalidResponse),
        }
    }

    fn calendar_delta_progress(
        &self,
        next_link: Option<String>,
        delta_link: Option<String>,
    ) -> Result<CalendarDeltaProgress, GraphError> {
        match (next_link, delta_link) {
            (Some(link), None) => Ok(CalendarDeltaProgress::Next(CalendarDeltaLink::from_opaque(
                self.authority.opaque_link(&link)?.as_str().to_owned(),
            ))),
            (None, Some(link)) => Ok(CalendarDeltaProgress::Complete(
                CalendarDeltaLink::from_opaque(
                    self.authority.opaque_link(&link)?.as_str().to_owned(),
                ),
            )),
            _ => Err(GraphError::InvalidResponse),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMailRequest<'a> {
    message: SendMailMessage<'a>,
    save_to_sent_items: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMailMessage<'a> {
    subject: &'a str,
    body: SendMailBody<'a>,
    to_recipients: [SendMailRecipient<'a>; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMailBody<'a> {
    content_type: &'static str,
    content: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMailRecipient<'a> {
    email_address: SendMailAddress<'a>,
}

#[derive(Serialize)]
struct SendMailAddress<'a> {
    address: &'a str,
}

impl<'a> From<&'a MailDraft> for SendMailRequest<'a> {
    fn from(draft: &'a MailDraft) -> Self {
        Self {
            message: SendMailMessage {
                subject: draft.subject(),
                body: SendMailBody {
                    content_type: "Text",
                    content: draft.body(),
                },
                to_recipients: [SendMailRecipient {
                    email_address: SendMailAddress {
                        address: draft.recipient().as_str(),
                    },
                }],
            },
            save_to_sent_items: true,
        }
    }
}

#[derive(Serialize)]
struct ReplyRequest<'a> {
    message: ReplyMessage<'a>,
}

#[derive(Serialize)]
struct ReplyMessage<'a> {
    body: SendMailBody<'a>,
}

fn mail_reply_body(draft: &MailReplyDraft) -> Result<Vec<u8>, GraphError> {
    serde_json::to_vec(&ReplyRequest {
        message: ReplyMessage {
            body: SendMailBody {
                content_type: "Text",
                content: draft.body(),
            },
        },
    })
    .map_err(|_| GraphError::InvalidRecord)
}

struct PageTraversal<'a> {
    api: &'a GraphApi,
    operation: GraphOperation,
    token: std::sync::Arc<AccessToken>,
    budget: PaginationBudget,
}

impl<'a> PageTraversal<'a> {
    fn new(
        api: &'a GraphApi,
        operation: GraphOperation,
        token: std::sync::Arc<AccessToken>,
    ) -> Self {
        Self {
            api,
            operation,
            token,
            budget: PaginationBudget::default(),
        }
    }

    async fn collect<T, U>(
        &mut self,
        start: GraphUrl,
        mut convert: impl FnMut(T) -> Result<U, GraphError>,
    ) -> Result<Vec<U>, GraphError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut next = start;
        let mut seen = HashSet::new();
        let mut records = Vec::new();
        loop {
            if !seen.insert(next.as_str().to_owned()) {
                return Err(GraphError::UnsafeLink);
            }
            let page = self.load_page(next).await?;
            for record in page.records {
                records.push(convert(record)?);
            }
            match page.next_link {
                Some(link) => next = self.api.authority.opaque_link(&link)?,
                None => return Ok(records),
            }
        }
    }

    async fn load_page<T>(&mut self, url: GraphUrl) -> Result<GraphPage<T>, GraphError>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.budget.begin_page()?;
        let body = self
            .api
            .transport
            .get(self.operation, url, std::sync::Arc::clone(&self.token))
            .await?;
        self.budget.add_response_bytes(body.len())?;
        serde_json::from_slice(&body).map_err(|_| GraphError::InvalidResponse)
    }
}

#[derive(Default)]
struct PaginationBudget {
    pages: usize,
    response_bytes: usize,
}

impl PaginationBudget {
    fn begin_page(&mut self) -> Result<(), GraphError> {
        if self.pages >= MAX_PAGINATION_PAGES {
            return Err(GraphError::PaginationLimitExceeded);
        }
        self.pages += 1;
        Ok(())
    }

    fn add_response_bytes(&mut self, bytes: usize) -> Result<(), GraphError> {
        let Some(total) = self.response_bytes.checked_add(bytes) else {
            return Err(GraphError::PaginationLimitExceeded);
        };
        if total > MAX_PAGINATION_RESPONSE_BYTES {
            return Err(GraphError::PaginationLimitExceeded);
        }
        self.response_bytes = total;
        Ok(())
    }
}

struct FolderTree {
    folders: Vec<MailFolder>,
    pending: VecDeque<FolderId>,
    seen: HashSet<FolderId>,
}

impl FolderTree {
    fn new() -> Self {
        Self {
            folders: Vec::new(),
            pending: VecDeque::new(),
            seen: HashSet::new(),
        }
    }

    fn add(&mut self, folders: Vec<MailFolder>) -> Result<(), GraphError> {
        for folder in folders {
            let folder_id = folder.reference().folder_id().clone();
            if !self.seen.insert(folder_id.clone()) {
                return Err(GraphError::InvalidRecord);
            }
            self.pending.push_back(folder_id);
            self.folders.push(folder);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphUserProfile {
    id: Option<String>,
    display_name: Option<String>,
    mail: Option<String>,
    user_principal_name: Option<String>,
}

impl TryFrom<GraphUserProfile> for AccountProfile {
    type Error = GraphError;

    fn try_from(profile: GraphUserProfile) -> Result<Self, Self::Error> {
        let id = profile
            .id
            .and_then(|id| AccountId::try_from(id).ok())
            .ok_or(GraphError::InvalidRecord)?;
        AccountProfile::try_new(
            id,
            profile.display_name,
            profile.mail,
            profile.user_principal_name,
        )
        .map_err(|_| GraphError::InvalidRecord)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphPage<T> {
    #[serde(rename = "value")]
    records: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphDeltaPage<T> {
    #[serde(rename = "value")]
    records: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    delta_link: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireFolder {
    id: String,
    display_name: String,
    #[serde(default)]
    is_hidden: bool,
}

#[derive(Deserialize)]
struct WireFolderId {
    id: String,
}

impl WireFolder {
    fn into_domain(
        self,
        account_id: &AccountId,
        parent_id: Option<FolderId>,
    ) -> Result<Option<MailFolder>, GraphError> {
        if self.is_hidden {
            return Ok(None);
        }
        let folder_id = FolderId::try_from(self.id).map_err(|_| GraphError::InvalidRecord)?;
        let reference = FolderReference::new(account_id.clone(), folder_id);
        let parent = parent_id.map(|folder_id| FolderReference::new(account_id.clone(), folder_id));
        MailFolder::new(reference, parent, self.display_name)
            .map(Some)
            .map_err(|_| GraphError::InvalidRecord)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireEmailAddress {
    address: String,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRecipient {
    email_address: WireEmailAddress,
}

impl TryFrom<WireRecipient> for Mailbox {
    type Error = GraphError;

    fn try_from(recipient: WireRecipient) -> Result<Self, Self::Error> {
        Mailbox::try_new(
            recipient.email_address.address,
            recipient.email_address.name,
        )
        .map_err(|_| GraphError::InvalidRecord)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMessage {
    id: String,
    subject: Option<String>,
    from: Option<WireRecipient>,
    sender: Option<WireRecipient>,
    received_date_time: String,
    is_read: bool,
    flag: Option<WireFollowUpFlag>,
}

impl WireMessage {
    fn into_domain(self, folder: FolderReference) -> Result<MailMessage, GraphError> {
        let id = MessageId::try_from(self.id).map_err(|_| GraphError::InvalidRecord)?;
        let received_at = OffsetDateTime::parse(&self.received_date_time, &Rfc3339)
            .map(SystemTime::from)
            .map_err(|_| GraphError::InvalidRecord)?;
        let from = self.from.map(Mailbox::try_from).transpose()?;
        let sender = self.sender.map(Mailbox::try_from).transpose()?;
        let origin = MessageOrigin::new(from, sender);
        let read_state = match self.is_read {
            true => ReadState::Read,
            false => ReadState::Unread,
        };
        let follow_up_flag_status = self.flag.and_then(|flag| flag.flag_status.map(Into::into));
        let metadata = MessageMetadata::new(
            self.subject,
            origin,
            received_at,
            read_state,
            follow_up_flag_status,
        );
        Ok(MailMessage::new(folder, id, metadata))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMessageChange {
    id: String,
    #[serde(flatten)]
    metadata: WireMessageMetadata,
    #[serde(rename = "@removed")]
    removed: Option<WireRemoved>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCalendarEvent {
    id: String,
    #[serde(default)]
    subject: WireField<String>,
    #[serde(default)]
    location: WireField<WireLocation>,
    #[serde(default)]
    start: WireField<WireDateTimeTimeZone>,
    #[serde(default)]
    end: WireField<WireDateTimeTimeZone>,
    #[serde(default)]
    is_all_day: WireField<bool>,
    #[serde(default)]
    original_start_time_zone: WireField<String>,
    #[serde(default)]
    original_end_time_zone: WireField<String>,
    #[serde(rename = "type")]
    event_type: Option<String>,
    is_organizer: Option<bool>,
    attendees: Option<Vec<serde::de::IgnoredAny>>,
    is_online_meeting: Option<bool>,
    change_key: Option<String>,
    #[serde(rename = "@odata.etag")]
    odata_etag: Option<String>,
    #[serde(rename = "@removed")]
    removed: Option<WireRemoved>,
}

struct CheckedEditableCalendarEvent {
    editable: EditableCalendarEvent,
    etag: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLocation {
    display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireDateTimeTimeZone {
    date_time: String,
    time_zone: String,
}

impl WireCalendarEvent {
    fn into_checked_editable(
        self,
        view: CalendarViewReference,
    ) -> Result<CheckedEditableCalendarEvent, GraphError> {
        let etag = self.odata_etag.clone();
        let editable = self.into_editable(view)?;
        Ok(CheckedEditableCalendarEvent { editable, etag })
    }

    fn into_editable(
        self,
        view: CalendarViewReference,
    ) -> Result<EditableCalendarEvent, GraphError> {
        if !self.has_supported_editable_shape() {
            return Err(GraphError::CalendarEventNotEditable);
        }
        let version = self
            .change_key
            .clone()
            .ok_or(GraphError::CalendarEventNotEditable)?;
        let version =
            CalendarEventVersion::try_from(version).map_err(|_| GraphError::InvalidRecord)?;
        let occurrence = self.into_new(view)?;
        EditableCalendarEvent::new(occurrence, version).map_err(|_| GraphError::InvalidRecord)
    }

    fn has_supported_editable_shape(&self) -> bool {
        self.event_type.as_deref() == Some("singleInstance")
            && self.is_organizer == Some(true)
            && self.attendees.as_ref().is_some_and(Vec::is_empty)
            && self.is_online_meeting == Some(false)
            && matches!(self.is_all_day, WireField::Value(false))
            && matches!(
                &self.subject,
                WireField::Value(subject)
                    if !subject.trim().is_empty()
                        && !subject.chars().any(char::is_control)
            )
    }

    fn into_change(self) -> Result<CalendarDeltaChange, GraphError> {
        let id = EventId::try_from(self.id).map_err(|_| GraphError::InvalidRecord)?;
        if self.removed.is_some() {
            return Ok(CalendarDeltaChange::Removed { id });
        }

        let mut patch = CalendarOccurrencePatch::new();
        apply_calendar_content_fields(self.subject, self.location, &mut patch);
        apply_calendar_zone_fields(
            self.original_start_time_zone,
            self.original_end_time_zone,
            &mut patch,
        );
        if let Some(time_patch) =
            calendar_occurrence_time_patch(self.start, self.end, self.is_all_day)?
        {
            patch.set_time(time_patch);
        }
        Ok(CalendarDeltaChange::Upsert { id, patch })
    }

    fn into_new(self, view: CalendarViewReference) -> Result<CalendarOccurrence, GraphError> {
        match self.into_change()? {
            CalendarDeltaChange::Upsert { id, patch } => patch
                .into_new(view, id)
                .map_err(|_| GraphError::InvalidRecord),
            CalendarDeltaChange::Removed { .. } => Err(GraphError::InvalidRecord),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateCalendarEventRequest<'a> {
    subject: &'a str,
    start: CreateEventDateTimeTimeZone,
    end: CreateEventDateTimeTimeZone,
    transaction_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateEventDateTimeTimeZone {
    date_time: String,
    time_zone: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CalendarEventUpdateRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<CreateEventDateTimeTimeZone>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end: Option<CreateEventDateTimeTimeZone>,
}

fn calendar_event_update_body(update: &CalendarEventUpdate) -> Result<Vec<u8>, GraphError> {
    let request = calendar_event_update_request(update)?;
    serde_json::to_vec(&request).map_err(|_| GraphError::InvalidRecord)
}

fn calendar_event_update_request(
    update: &CalendarEventUpdate,
) -> Result<CalendarEventUpdateRequest<'_>, GraphError> {
    let source = update.source().occurrence();
    let subject =
        (source.subject() != Some(update.draft().subject())).then_some(update.draft().subject());
    let (start, end) = calendar_event_update_times(update, source)?;
    if subject.is_none() && start.is_none() {
        return Err(GraphError::InvalidRecord);
    }
    Ok(CalendarEventUpdateRequest {
        subject,
        start,
        end,
    })
}

fn calendar_event_update_times(
    update: &CalendarEventUpdate,
    source: &CalendarOccurrence,
) -> Result<
    (
        Option<CreateEventDateTimeTimeZone>,
        Option<CreateEventDateTimeTimeZone>,
    ),
    GraphError,
> {
    if source.time() == update.draft().time() {
        return Ok((None, None));
    }
    let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
        return Err(GraphError::InvalidRecord);
    };
    Ok((
        Some(CreateEventDateTimeTimeZone {
            date_time: graph_utc_datetime(*start)?,
            time_zone: "UTC",
        }),
        Some(CreateEventDateTimeTimeZone {
            date_time: graph_utc_datetime(*end)?,
            time_zone: "UTC",
        }),
    ))
}

fn confirm_calendar_event_update(
    update: &CalendarEventUpdate,
    current: &EditableCalendarEvent,
    response: &[u8],
) -> Result<CalendarOccurrence, GraphError> {
    let event_id = update.source().occurrence().id();
    let patch = calendar_event_update_patch(response, event_id)?;
    let mut event = current.occurrence().clone();
    patch
        .apply_to(&mut event)
        .map_err(|_| GraphError::CalendarEventUpdateOutcomeUnknown)?;
    if update.confirms(&event) {
        Ok(event)
    } else {
        Err(GraphError::CalendarEventUpdateOutcomeUnknown)
    }
}

fn calendar_event_update_patch(
    response: &[u8],
    expected_event_id: &EventId,
) -> Result<CalendarOccurrencePatch, GraphError> {
    let patch = serde_json::from_slice::<WireCalendarEvent>(response)
        .map_err(|_| GraphError::CalendarEventUpdateOutcomeUnknown)?
        .into_change()
        .map_err(|_| GraphError::CalendarEventUpdateOutcomeUnknown)?;
    let CalendarDeltaChange::Upsert { id, patch } = patch else {
        return Err(GraphError::CalendarEventUpdateOutcomeUnknown);
    };
    if &id != expected_event_id {
        return Err(GraphError::CalendarEventUpdateOutcomeUnknown);
    }
    Ok(patch)
}

fn create_calendar_event_body(creation: &CalendarEventCreation) -> Result<Vec<u8>, GraphError> {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        return Err(GraphError::InvalidRecord);
    };
    let request = CreateCalendarEventRequest {
        subject: creation.draft().subject(),
        start: CreateEventDateTimeTimeZone {
            date_time: graph_utc_datetime(*start)?,
            time_zone: "UTC",
        },
        end: CreateEventDateTimeTimeZone {
            date_time: graph_utc_datetime(*end)?,
            time_zone: "UTC",
        },
        transaction_id: creation.id().as_uuid().to_string(),
    };
    serde_json::to_vec(&request).map_err(|_| GraphError::InvalidRecord)
}

fn graph_utc_datetime(value: OffsetDateTime) -> Result<String, GraphError> {
    if value.offset() != UtcOffset::UTC {
        return Err(GraphError::InvalidRecord);
    }
    value
        .format(&Rfc3339)
        .map_err(|_| GraphError::InvalidRecord)?
        .strip_suffix('Z')
        .map(str::to_owned)
        .ok_or(GraphError::InvalidRecord)
}

fn apply_calendar_content_fields(
    subject: WireField<String>,
    location: WireField<WireLocation>,
    patch: &mut CalendarOccurrencePatch,
) {
    match subject {
        WireField::Missing => {}
        WireField::Null => patch.set_subject(None),
        WireField::Value(subject) => patch.set_subject(Some(subject)),
    }
    match location {
        WireField::Missing => {}
        WireField::Null => patch.set_location(None),
        WireField::Value(location) => patch.set_location(location.display_name),
    }
}

fn apply_calendar_zone_fields(
    original_start_time_zone: WireField<String>,
    original_end_time_zone: WireField<String>,
    patch: &mut CalendarOccurrencePatch,
) {
    match original_start_time_zone {
        WireField::Missing => {}
        WireField::Null => patch.set_original_start_time_zone(None),
        WireField::Value(time_zone) => {
            patch.set_original_start_time_zone(Some(time_zone));
        }
    }
    match original_end_time_zone {
        WireField::Missing => {}
        WireField::Null => patch.set_original_end_time_zone(None),
        WireField::Value(time_zone) => {
            patch.set_original_end_time_zone(Some(time_zone));
        }
    }
}

fn calendar_occurrence_time_patch(
    start: WireField<WireDateTimeTimeZone>,
    end: WireField<WireDateTimeTimeZone>,
    is_all_day: WireField<bool>,
) -> Result<Option<CalendarOccurrenceTimePatch>, GraphError> {
    let mut time_patch = CalendarOccurrenceTimePatch::new();
    let start = parse_optional_calendar_time(start)?;
    let end = parse_optional_calendar_time(end)?;
    if let Some(start) = start {
        time_patch.set_start(start);
    }
    if let Some(end) = end {
        time_patch.set_end(end);
    }
    let has_all_day = apply_calendar_all_day_field(is_all_day, &mut time_patch)?;
    Ok((start.is_some() || end.is_some() || has_all_day).then_some(time_patch))
}

fn apply_calendar_all_day_field(
    field: WireField<bool>,
    time_patch: &mut CalendarOccurrenceTimePatch,
) -> Result<bool, GraphError> {
    match field {
        WireField::Missing => Ok(false),
        WireField::Null => Err(GraphError::InvalidRecord),
        WireField::Value(is_all_day) => {
            time_patch.set_is_all_day(is_all_day);
            Ok(true)
        }
    }
}

fn parse_optional_calendar_time(
    field: WireField<WireDateTimeTimeZone>,
) -> Result<Option<OffsetDateTime>, GraphError> {
    match field {
        WireField::Missing => Ok(None),
        WireField::Null => Err(GraphError::InvalidRecord),
        WireField::Value(date_time) => parse_graph_utc_date_time(&date_time).map(Some),
    }
}

fn parse_graph_utc_date_time(
    date_time: &WireDateTimeTimeZone,
) -> Result<OffsetDateTime, GraphError> {
    if date_time.time_zone != "UTC" {
        return Err(GraphError::InvalidRecord);
    }
    let date_time_utc = format!("{}Z", date_time.date_time);
    OffsetDateTime::parse(&date_time_utc, &Rfc3339).map_err(|_| GraphError::InvalidRecord)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMessageMetadata {
    #[serde(default)]
    subject: WireField<String>,
    #[serde(default)]
    from: WireField<WireRecipient>,
    #[serde(default)]
    sender: WireField<WireRecipient>,
    #[serde(default)]
    received_date_time: WireField<String>,
    #[serde(default)]
    is_read: WireField<bool>,
    #[serde(default)]
    flag: WireField<WireFollowUpFlag>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireFollowUpFlag {
    flag_status: Option<WireFollowUpFlagStatus>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum WireFollowUpFlagStatus {
    NotFlagged,
    Flagged,
    Complete,
}

impl From<WireFollowUpFlagStatus> for FollowUpFlagStatus {
    fn from(status: WireFollowUpFlagStatus) -> Self {
        match status {
            WireFollowUpFlagStatus::NotFlagged => Self::NotFlagged,
            WireFollowUpFlagStatus::Flagged => Self::Flagged,
            WireFollowUpFlagStatus::Complete => Self::Complete,
        }
    }
}

#[derive(Default)]
enum WireField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T> Deserialize<'de> for WireField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FieldVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T> serde::de::Visitor<'de> for FieldVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = WireField<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Graph property value or null")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(WireField::Null)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(WireField::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                T::deserialize(deserializer).map(WireField::Value)
            }
        }

        deserializer.deserialize_option(FieldVisitor(std::marker::PhantomData))
    }
}

struct WireRemoved {
    _reason: Option<String>,
}

impl<'de> Deserialize<'de> for WireRemoved {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RemovedFields {
            reason: Option<String>,
        }

        let removed = RemovedFields::deserialize(deserializer)?;
        Ok(Self {
            _reason: removed.reason,
        })
    }
}

impl TryFrom<WireMessageChange> for MessageDeltaChange {
    type Error = GraphError;

    fn try_from(change: WireMessageChange) -> Result<Self, Self::Error> {
        let id = MessageId::try_from(change.id).map_err(|_| GraphError::InvalidRecord)?;
        if change.removed.is_some() {
            return Ok(Self::Removed { id });
        }
        let mut patch = MessageMetadataPatch::new();
        apply_nullable_fields(
            change.metadata.subject,
            change.metadata.from,
            change.metadata.sender,
            &mut patch,
        )?;
        apply_required_fields(
            change.metadata.received_date_time,
            change.metadata.is_read,
            &mut patch,
        )?;
        apply_follow_up_flag_field(change.metadata.flag, &mut patch);
        Ok(Self::Upsert { id, patch })
    }
}

fn apply_nullable_fields(
    subject: WireField<String>,
    from: WireField<WireRecipient>,
    sender: WireField<WireRecipient>,
    patch: &mut MessageMetadataPatch,
) -> Result<(), GraphError> {
    match subject {
        WireField::Missing => {}
        WireField::Null => patch.set_subject(None),
        WireField::Value(subject) => patch.set_subject(Some(subject)),
    }
    match from {
        WireField::Missing => {}
        WireField::Null => patch.set_from(None),
        WireField::Value(from) => patch.set_from(Some(Mailbox::try_from(from)?)),
    }
    match sender {
        WireField::Missing => {}
        WireField::Null => patch.set_sender(None),
        WireField::Value(sender) => patch.set_sender(Some(Mailbox::try_from(sender)?)),
    }
    Ok(())
}

fn apply_required_fields(
    received_date_time: WireField<String>,
    is_read: WireField<bool>,
    patch: &mut MessageMetadataPatch,
) -> Result<(), GraphError> {
    match received_date_time {
        WireField::Missing => {}
        WireField::Null => return Err(GraphError::InvalidRecord),
        WireField::Value(received_date_time) => {
            let received_at = OffsetDateTime::parse(&received_date_time, &Rfc3339)
                .map(SystemTime::from)
                .map_err(|_| GraphError::InvalidRecord)?;
            patch.set_received_at(received_at);
        }
    }
    match is_read {
        WireField::Missing => {}
        WireField::Null => return Err(GraphError::InvalidRecord),
        WireField::Value(true) => patch.set_read_state(ReadState::Read),
        WireField::Value(false) => patch.set_read_state(ReadState::Unread),
    }
    Ok(())
}

fn apply_follow_up_flag_field(flag: WireField<WireFollowUpFlag>, patch: &mut MessageMetadataPatch) {
    match flag {
        WireField::Missing => {}
        WireField::Null => patch.set_follow_up_flag_status(None),
        WireField::Value(flag) => {
            patch.set_follow_up_flag_status(flag.flag_status.map(Into::into));
        }
    }
}

#[derive(Deserialize)]
struct WireMessageBody {
    body: WireBodyContent,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMessageReadState {
    is_read: bool,
}

fn read_state_patch_body(read_state: ReadState) -> Arc<[u8]> {
    match read_state {
        ReadState::Read => Arc::from(&b"{\"isRead\":true}"[..]),
        ReadState::Unread => Arc::from(&b"{\"isRead\":false}"[..]),
    }
}

fn confirmed_read_state(response: &[u8], requested: ReadState) -> Result<ReadState, GraphError> {
    let message = serde_json::from_slice::<WireMessageReadState>(response)
        .map_err(|_| GraphError::MessageUpdateOutcomeUnknown)?;
    let confirmed = if message.is_read {
        ReadState::Read
    } else {
        ReadState::Unread
    };
    if confirmed != requested {
        return Err(GraphError::MessageUpdateOutcomeUnknown);
    }
    Ok(confirmed)
}

fn follow_up_flag_patch_body(status: FollowUpFlagStatus) -> Arc<[u8]> {
    match status {
        FollowUpFlagStatus::NotFlagged => {
            Arc::from(&b"{\"flag\":{\"flagStatus\":\"notFlagged\"}}"[..])
        }
        FollowUpFlagStatus::Flagged => Arc::from(&b"{\"flag\":{\"flagStatus\":\"flagged\"}}"[..]),
        FollowUpFlagStatus::Complete => Arc::from(&b"{\"flag\":{\"flagStatus\":\"complete\"}}"[..]),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireMessageMoveRequest<'a> {
    destination_id: &'a str,
}

fn message_move_body(destination_id: &str) -> Result<Arc<[u8]>, GraphError> {
    serde_json::to_vec(&WireMessageMoveRequest { destination_id })
        .map(Arc::from)
        .map_err(|_| GraphError::InvalidRecord)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMovedMessage {
    id: String,
    parent_folder_id: String,
}

fn confirmed_message_move(
    response: &[u8],
    requested_message_id: &MessageId,
    requested_destination_id: &str,
) -> Result<(), GraphError> {
    let moved = serde_json::from_slice::<WireMovedMessage>(response)
        .map_err(|_| GraphError::MessageMoveOutcomeUnknown)?;
    if moved.id != requested_message_id.as_str()
        || moved.parent_folder_id != requested_destination_id
    {
        return Err(GraphError::MessageMoveOutcomeUnknown);
    }
    Ok(())
}

#[derive(Deserialize)]
struct WireMessageFollowUpFlagUpdate {
    flag: WireFollowUpFlag,
}

fn confirmed_follow_up_flag_status(
    response: &[u8],
    requested: FollowUpFlagStatus,
) -> Result<FollowUpFlagStatus, GraphError> {
    let message = serde_json::from_slice::<WireMessageFollowUpFlagUpdate>(response)
        .map_err(|_| GraphError::MessageUpdateOutcomeUnknown)?;
    let confirmed = message
        .flag
        .flag_status
        .map(Into::into)
        .ok_or(GraphError::MessageUpdateOutcomeUnknown)?;
    if confirmed != requested {
        return Err(GraphError::MessageUpdateOutcomeUnknown);
    }
    Ok(confirmed)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireBodyContent {
    content_type: String,
    content: String,
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::SystemTime};

    use httpmock::{
        Method::{DELETE, GET, PATCH, POST},
        MockServer,
    };
    use huskarl::{
        core::secrets::SecretString,
        token::{AccessToken, BearerAccessToken},
    };
    use serde_json::json;
    use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};
    use url::Url;

    use einfach_mail_application::CalendarDeltaRequest;
    use einfach_mail_domain::{
        AccountId, CalendarDeltaChange, CalendarDeltaProgress, CalendarEventCreation,
        CalendarEventDraft, CalendarEventUpdate, CalendarEventUpdateDraft, CalendarOccurrence,
        CalendarOccurrenceTime, CalendarTimeZoneId, CalendarViewRange, CalendarViewReference,
        EditableCalendarEvent, FolderId, FolderReference, FollowUpFlagStatus, MailDraft,
        MailRecipient, MailReplyDraft, MailReplySource, MailSubject, MessageDeltaChange, MessageId,
        MessageMetadata, MessageOrigin, ReadState,
    };

    use super::{
        GraphApi, MAX_PAGINATION_PAGES, MAX_PAGINATION_RESPONSE_BYTES, PaginationBudget,
        WireCalendarEvent, WireFolder, WireMessage, WireMessageChange, calendar_event_update_body,
        confirmed_follow_up_flag_status, confirmed_message_move, confirmed_read_state,
        create_calendar_event_body, follow_up_flag_patch_body, mail_reply_body, message_move_body,
        read_state_patch_body,
    };
    use crate::{
        error::GraphError,
        graph_url::{CALENDAR_EVENT_EDIT_SELECT, GraphAuthority},
        transport::{GraphTransport, GraphTransportHandle},
    };

    fn synthetic_access_token() -> Arc<AccessToken> {
        Arc::new(AccessToken::Bearer(BearerAccessToken::new(
            SecretString::new("synthetic-calendar-access-token"),
            SystemTime::now(),
            Some(std::time::Duration::from_secs(3600)),
        )))
    }

    fn synthetic_view() -> CalendarViewReference {
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let start_date = Date::from_calendar_date(2030, Month::January, 7)
            .expect("synthetic week start date is valid");
        let end_date = Date::from_calendar_date(2030, Month::January, 14)
            .expect("synthetic week end date is valid");
        let range = CalendarViewRange::new(
            start_date.midnight().assume_utc(),
            end_date.midnight().assume_utc(),
            start_date,
            end_date,
        )
        .expect("synthetic calendar range is valid");
        CalendarViewReference::new(account, range)
    }

    #[test]
    fn mail_reply_body_contains_only_plain_text_message_body() {
        let account =
            AccountId::try_from("synthetic-account".to_owned()).expect("account ID is valid");
        let folder = FolderReference::new(
            account,
            FolderId::try_from("inbox".to_owned()).expect("folder ID is valid"),
        );
        let source = MailReplySource::new(
            folder,
            MessageId::try_from("source-message".to_owned()).expect("message ID is valid"),
        );
        let draft = MailReplyDraft::new(source, "Synthetic reply\nSecond line".to_owned());
        let serialized = mail_reply_body(&draft).expect("reply request serializes");
        let request: serde_json::Value =
            serde_json::from_slice(&serialized).expect("reply request is valid JSON");

        assert_eq!(request["message"]["body"]["contentType"], "Text");
        assert_eq!(
            request["message"]["body"]["content"],
            "Synthetic reply\nSecond line"
        );
        assert!(request["message"].get("toRecipients").is_none());
        assert!(request["message"].get("subject").is_none());
        assert!(request.get("comment").is_none());
    }

    fn editable_event_json(
        version: &str,
        subject: &str,
        start: &str,
        end: &str,
    ) -> serde_json::Value {
        json!({
            "id": "synthetic-event",
            "subject": subject,
            "location": {"displayName": "Room A"},
            "start": {"dateTime": start, "timeZone": "UTC"},
            "end": {"dateTime": end, "timeZone": "UTC"},
            "isAllDay": false,
            "originalStartTimeZone": "UTC",
            "originalEndTimeZone": "UTC",
            "type": "singleInstance",
            "isOrganizer": true,
            "attendees": [],
            "isOnlineMeeting": false,
            "changeKey": version,
            "@odata.etag": "W/\"synthetic-etag-1\""
        })
    }

    fn editable_event(version: &str) -> EditableCalendarEvent {
        serde_json::from_value::<WireCalendarEvent>(editable_event_json(
            version,
            "Planning",
            "2030-01-08T09:00:00.0000000",
            "2030-01-08T10:00:00.0000000",
        ))
        .expect("synthetic event metadata deserializes")
        .into_editable(synthetic_view())
        .expect("synthetic single-instance appointment is editable")
    }

    fn utc_time(day: u8, hour: u8) -> OffsetDateTime {
        Date::from_calendar_date(2030, Month::January, day)
            .expect("synthetic event date is valid")
            .with_hms(hour, 0, 0)
            .expect("synthetic event time is valid")
            .assume_utc()
    }

    fn synthetic_graph_api(server: &MockServer) -> (GraphApi, GraphTransport) {
        let endpoint =
            Url::parse(&server.url("/v1.0/me")).expect("synthetic Graph profile endpoint is valid");
        let authority = GraphAuthority::from_profile_endpoint(&endpoint)
            .expect("synthetic Graph authority is valid");
        let transport =
            GraphTransport::start_for_fixtures().expect("synthetic Graph transport worker starts");
        let handle: GraphTransportHandle = transport.handle();
        (GraphApi::new(authority, handle), transport)
    }

    #[tokio::test]
    async fn move_message_sends_one_immutable_id_request_and_checks_destination() {
        let server = MockServer::start_async().await;
        let post = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/v1.0/me/messages/synthetic-message/move")
                    .header("prefer", "IdType=\"ImmutableId\"")
                    .header("content-type", "application/json")
                    .body(r#"{"destinationId":"archive"}"#);
                then.status(201)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"synthetic-message","parentFolderId":"archive"}"#);
            })
            .await;
        let (graph_api, transport) = synthetic_graph_api(&server);
        let message_id = MessageId::try_from("synthetic-message".to_owned())
            .expect("synthetic message ID is valid");
        let destination = FolderReference::new(
            AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is valid"),
            FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
        );

        let result = graph_api
            .move_message(&message_id, &destination, synthetic_access_token())
            .await;

        assert_eq!(result, Ok(()));
        post.assert_calls_async(1).await;
        drop(transport);
    }

    #[tokio::test]
    async fn resolves_deleted_items_from_its_well_known_name() {
        let server = MockServer::start_async().await;
        let lookup = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/mailFolders/deleteditems")
                    .query_param("$select", "id");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"synthetic-deleted-folder"}"#);
            })
            .await;
        let (graph_api, transport) = synthetic_graph_api(&server);

        let folder_id = graph_api
            .deleted_items_folder_id(synthetic_access_token())
            .await
            .expect("synthetic Deleted Items folder ID is returned");

        assert_eq!(folder_id.as_str(), "synthetic-deleted-folder");
        lookup.assert_calls_async(1).await;
        drop(transport);
    }

    #[test]
    fn message_move_confirmation_requires_matching_message_and_folder() {
        let message_id = MessageId::try_from("synthetic-message".to_owned())
            .expect("synthetic message ID is valid");
        assert_eq!(
            message_move_body("archive")
                .expect("synthetic move body serializes")
                .as_ref(),
            br#"{"destinationId":"archive"}"#
        );
        assert_eq!(
            confirmed_message_move(
                br#"{"id":"synthetic-message","parentFolderId":"archive"}"#,
                &message_id,
                "archive"
            ),
            Ok(())
        );
        for response in [
            br#"{}"#.as_slice(),
            br#"{"id":"different-message","parentFolderId":"archive"}"#,
            br#"{"id":"synthetic-message","parentFolderId":"inbox"}"#,
        ] {
            assert_eq!(
                confirmed_message_move(response, &message_id, "archive"),
                Err(GraphError::MessageMoveOutcomeUnknown)
            );
        }
    }

    #[test]
    fn read_state_patch_is_minimal_and_requires_server_confirmation() {
        assert_eq!(
            read_state_patch_body(ReadState::Read).as_ref(),
            br#"{"isRead":true}"#
        );
        assert_eq!(
            read_state_patch_body(ReadState::Unread).as_ref(),
            br#"{"isRead":false}"#
        );
        assert_eq!(
            confirmed_read_state(br#"{"isRead":true}"#, ReadState::Read),
            Ok(ReadState::Read)
        );
        assert_eq!(
            confirmed_read_state(br#"{"isRead":false}"#, ReadState::Read),
            Err(GraphError::MessageUpdateOutcomeUnknown)
        );
        assert_eq!(
            confirmed_read_state(br#"{}"#, ReadState::Read),
            Err(GraphError::MessageUpdateOutcomeUnknown)
        );
    }

    #[test]
    fn follow_up_flag_patch_is_minimal_and_requires_matching_confirmation() {
        for (status, payload) in [
            (
                FollowUpFlagStatus::NotFlagged,
                br#"{"flag":{"flagStatus":"notFlagged"}}"#.as_slice(),
            ),
            (
                FollowUpFlagStatus::Flagged,
                br#"{"flag":{"flagStatus":"flagged"}}"#.as_slice(),
            ),
            (
                FollowUpFlagStatus::Complete,
                br#"{"flag":{"flagStatus":"complete"}}"#.as_slice(),
            ),
        ] {
            assert_eq!(follow_up_flag_patch_body(status).as_ref(), payload);
            assert_eq!(confirmed_follow_up_flag_status(payload, status), Ok(status));
        }

        assert_eq!(
            confirmed_follow_up_flag_status(br#"{}"#, FollowUpFlagStatus::Flagged),
            Err(GraphError::MessageUpdateOutcomeUnknown)
        );
        assert_eq!(
            confirmed_follow_up_flag_status(
                br#"{"flag":{"flagStatus":"complete"}}"#,
                FollowUpFlagStatus::Flagged
            ),
            Err(GraphError::MessageUpdateOutcomeUnknown)
        );
        assert!(
            serde_json::from_slice::<WireMessageChange>(
                br#"{"id":"synthetic-message","flag":{"flagStatus":"unknown"}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn delta_flag_updates_preserve_omitted_status_and_clear_explicit_null() {
        let mut metadata = MessageMetadata::new(
            None,
            MessageOrigin::new(None, None),
            SystemTime::UNIX_EPOCH,
            ReadState::Unread,
            Some(FollowUpFlagStatus::Flagged),
        );

        for (payload, expected) in [
            (
                br#"{"id":"synthetic-message"}"#.as_slice(),
                Some(FollowUpFlagStatus::Flagged),
            ),
            (
                br#"{"id":"synthetic-message","flag":{"flagStatus":"complete"}}"#.as_slice(),
                Some(FollowUpFlagStatus::Complete),
            ),
            (
                br#"{"id":"synthetic-message","flag":null}"#.as_slice(),
                None,
            ),
            (
                br#"{"id":"synthetic-message","flag":{"flagStatus":null}}"#.as_slice(),
                None,
            ),
        ] {
            let wire = serde_json::from_slice::<WireMessageChange>(payload)
                .expect("synthetic message delta is valid");
            let MessageDeltaChange::Upsert { patch, .. } =
                MessageDeltaChange::try_from(wire).expect("message delta maps to domain values")
            else {
                panic!("synthetic metadata change is an upsert");
            };
            metadata.apply_patch(patch);
            assert_eq!(metadata.follow_up_flag_status(), expected);
        }
    }

    #[tokio::test]
    async fn follow_up_flag_patch_uses_the_message_update_endpoint() {
        let server = MockServer::start_async().await;
        let patch = server
            .mock_async(|when, then| {
                when.method(PATCH)
                    .path("/v1.0/me/messages/synthetic-message")
                    .header("content-type", "application/json")
                    .header("prefer", "IdType=\"ImmutableId\"")
                    .body(r#"{"flag":{"flagStatus":"flagged"}}"#);
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"flag":{"flagStatus":"flagged"}}"#);
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);
        let message_id = MessageId::try_from("synthetic-message".to_owned())
            .expect("synthetic message ID is valid");

        assert_eq!(
            api.set_message_follow_up_flag_status(
                &message_id,
                FollowUpFlagStatus::Flagged,
                synthetic_access_token(),
            )
            .await,
            Ok(FollowUpFlagStatus::Flagged)
        );

        patch.assert_async().await;
    }

    #[tokio::test]
    async fn read_state_update_sends_minimal_patch_and_returns_exact_confirmation() {
        let server = MockServer::start_async().await;
        let patch = server
            .mock_async(|when, then| {
                when.method(PATCH)
                    .path("/v1.0/me/messages/synthetic-message")
                    .header("content-type", "application/json")
                    .header("prefer", "IdType=\"ImmutableId\"")
                    .body(r#"{"isRead":true}"#);
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"isRead":true}"#);
            })
            .await;
        let (graph_api, _transport) = synthetic_graph_api(&server);

        let confirmed = graph_api
            .set_message_read_state(
                &MessageId::try_from("synthetic-message".to_owned())
                    .expect("synthetic message ID is valid"),
                ReadState::Read,
                synthetic_access_token(),
            )
            .await
            .expect("Graph confirms the requested read state");

        assert_eq!(confirmed, ReadState::Read);
        patch.assert_async().await;
    }

    #[tokio::test]
    async fn send_mail_posts_one_plain_text_recipient_and_saves_to_sent_items() {
        let server = MockServer::start_async().await;
        let recipient = "ada@example.test"
            .parse::<MailRecipient>()
            .expect("synthetic recipient is valid");
        let subject = "Synthetic subject"
            .parse::<MailSubject>()
            .expect("synthetic subject is valid");
        let draft = MailDraft::new(recipient, subject, "Synthetic body".to_owned());
        let expected_body = json!({
            "message": {
                "subject": "Synthetic subject",
                "body": { "contentType": "Text", "content": "Synthetic body" },
                "toRecipients": [{
                    "emailAddress": { "address": "ada@example.test" }
                }]
            },
            "saveToSentItems": true
        });
        let accepted = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/v1.0/me/sendMail")
                    .json_body(expected_body);
                then.status(202);
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);

        api.send_mail(&draft, synthetic_access_token())
            .await
            .expect("Graph accepts the synthetic plain-text message");

        accepted.assert_async().await;
    }

    #[test]
    fn create_event_request_contains_only_timed_primary_calendar_fields() {
        let view = synthetic_view();
        let date = Date::from_calendar_date(2030, Month::January, 8)
            .expect("synthetic event date is valid");
        let local_start = PrimitiveDateTime::new(
            date,
            Time::from_hms(9, 30, 0).expect("synthetic start time is valid"),
        );
        let local_end = PrimitiveDateTime::new(
            date,
            Time::from_hms(10, 0, 0).expect("synthetic end time is valid"),
        );
        let draft = CalendarEventDraft::timed(
            "Synthetic planning meeting".to_owned(),
            local_start,
            local_end,
            CalendarTimeZoneId::try_from("Etc/UTC".to_owned())
                .expect("synthetic time zone is valid"),
            date.with_hms(9, 30, 0)
                .expect("synthetic UTC start is valid")
                .assume_utc(),
            date.with_hms(10, 0, 0)
                .expect("synthetic UTC end is valid")
                .assume_utc(),
        )
        .expect("synthetic event draft is valid");
        let creation =
            CalendarEventCreation::try_new(view, draft).expect("synthetic event fits its week");

        let body = create_calendar_event_body(&creation).expect("synthetic event body serializes");
        let body: serde_json::Value =
            serde_json::from_slice(&body).expect("synthetic event body is JSON");
        let object = body
            .as_object()
            .expect("synthetic event body is a JSON object");
        let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort_unstable();

        assert_eq!(keys, ["end", "start", "subject", "transactionId"]);
        assert_eq!(body["subject"], "Synthetic planning meeting");
        assert_eq!(body["start"]["dateTime"], "2030-01-08T09:30:00");
        assert_eq!(body["start"]["timeZone"], "UTC");
        assert_eq!(body["end"]["dateTime"], "2030-01-08T10:00:00");
        assert_eq!(body["end"]["timeZone"], "UTC");
        assert_eq!(body["transactionId"], creation.id().as_uuid().to_string());
    }

    #[test]
    fn event_update_body_contains_only_changed_fields() {
        let subject_update = CalendarEventUpdate::new(
            editable_event("synthetic-version-1"),
            CalendarEventUpdateDraft::timed(
                "Updated planning".to_owned(),
                utc_time(8, 9),
                utc_time(8, 10),
            )
            .expect("synthetic subject update is valid"),
        )
        .expect("synthetic subject changed");
        let subject_body: serde_json::Value = serde_json::from_slice(
            &calendar_event_update_body(&subject_update)
                .expect("synthetic subject update serializes"),
        )
        .expect("subject update is JSON");
        assert_eq!(subject_body, json!({"subject": "Updated planning"}));

        let time_update = CalendarEventUpdate::new(
            editable_event("synthetic-version-1"),
            CalendarEventUpdateDraft::timed(
                "Planning".to_owned(),
                utc_time(8, 11),
                utc_time(8, 12),
            )
            .expect("synthetic time update is valid"),
        )
        .expect("synthetic time changed");
        let time_body: serde_json::Value = serde_json::from_slice(
            &calendar_event_update_body(&time_update).expect("synthetic time update serializes"),
        )
        .expect("time update is JSON");
        assert_eq!(
            time_body,
            json!({
                "start": {"dateTime": "2030-01-08T11:00:00", "timeZone": "UTC"},
                "end": {"dateTime": "2030-01-08T12:00:00", "timeZone": "UTC"}
            })
        );
    }

    #[test]
    fn event_edit_boundary_rejects_meetings_recurring_events_and_unknown_metadata() {
        let valid_event_json = editable_event_json(
            "synthetic-version-1",
            "Planning",
            "2030-01-08T09:00:00.0000000",
            "2030-01-08T10:00:00.0000000",
        );
        for event_json in [
            {
                let mut event_json = valid_event_json.clone();
                event_json["type"] = json!("occurrence");
                event_json
            },
            {
                let mut event_json = valid_event_json.clone();
                event_json["isOrganizer"] = json!(false);
                event_json
            },
            {
                let mut event_json = valid_event_json.clone();
                event_json["attendees"] = json!([{}]);
                event_json
            },
            {
                let mut event_json = valid_event_json.clone();
                event_json["isOnlineMeeting"] = json!(true);
                event_json
            },
            {
                let mut event_json = valid_event_json.clone();
                event_json["isAllDay"] = json!(true);
                event_json
            },
            {
                let mut event_json = valid_event_json.clone();
                event_json["subject"] = serde_json::Value::Null;
                event_json
            },
            {
                let mut event_json = valid_event_json.clone();
                event_json["subject"] = json!("");
                event_json
            },
            {
                let mut event_json = valid_event_json.clone();
                event_json["subject"] = json!("Planning\tmeeting");
                event_json
            },
            {
                let mut event_json = valid_event_json.clone();
                event_json
                    .as_object_mut()
                    .expect("synthetic event is an object")
                    .remove("changeKey");
                event_json
            },
        ] {
            let wire = serde_json::from_value::<WireCalendarEvent>(event_json)
                .expect("synthetic event metadata deserializes");
            assert_eq!(
                wire.into_editable(synthetic_view()),
                Err(GraphError::CalendarEventNotEditable)
            );
        }
    }

    #[tokio::test]
    async fn event_update_rechecks_version_and_patches_only_requested_fields() {
        let server = MockServer::start_async().await;
        let view = synthetic_view();
        let source = serde_json::from_value::<WireCalendarEvent>(editable_event_json(
            "synthetic-version-1",
            "Planning",
            "2030-01-08T09:00:00.0000000",
            "2030-01-08T10:00:00.0000000",
        ))
        .expect("synthetic event deserializes")
        .into_new(view)
        .expect("synthetic cached event is complete");
        let updated = editable_event_json(
            "synthetic-version-2",
            "Updated planning",
            "2030-01-08T11:00:00.0000000",
            "2030-01-08T12:00:00.0000000",
        );
        let read = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/events/synthetic-event")
                    .header("prefer", "IdType=\"ImmutableId\"")
                    .query_param("$select", CALENDAR_EVENT_EDIT_SELECT);
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        editable_event_json(
                            "synthetic-version-1",
                            "Planning",
                            "2030-01-08T09:00:00.0000000",
                            "2030-01-08T10:00:00.0000000",
                        )
                        .to_string(),
                    );
            })
            .await;
        let patch = server
            .mock_async(|when, then| {
                when.method(PATCH)
                    .path("/v1.0/me/events/synthetic-event")
                    .header("prefer", "IdType=\"ImmutableId\"")
                    .query_param_missing("$select")
                    .header("content-type", "application/json")
                    .body(r#"{"subject":"Updated planning","start":{"dateTime":"2030-01-08T11:00:00","timeZone":"UTC"},"end":{"dateTime":"2030-01-08T12:00:00","timeZone":"UTC"}}"#);
                then.status(200)
                    .header("content-type", "application/json")
                    .body(updated.to_string());
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);
        let editable = api
            .editable_calendar_event(&source, synthetic_access_token())
            .await
            .expect("synthetic supported event passes the edit check");
        let update = CalendarEventUpdate::new(
            editable,
            CalendarEventUpdateDraft::timed(
                "Updated planning".to_owned(),
                utc_time(8, 11),
                utc_time(8, 12),
            )
            .expect("synthetic update draft is valid"),
        )
        .expect("synthetic event values changed");

        let saved = api
            .update_calendar_event(&update, synthetic_access_token())
            .await
            .expect("Graph confirms the exact event update");

        assert_eq!(saved.subject(), Some("Updated planning"));
        assert_eq!(saved.location(), Some("Room A"));
        assert_eq!(saved.time(), update.draft().time());
        read.assert_calls_async(2).await;
        patch.assert_async().await;
    }

    #[tokio::test]
    async fn event_update_refuses_a_changed_version_without_patching() {
        let server = MockServer::start_async().await;
        let source = editable_event("synthetic-version-1");
        let update = CalendarEventUpdate::new(
            source,
            CalendarEventUpdateDraft::timed(
                "Updated planning".to_owned(),
                utc_time(8, 11),
                utc_time(8, 12),
            )
            .expect("synthetic update draft is valid"),
        )
        .expect("synthetic event values changed");
        let read = server
            .mock_async(|when, then| {
                when.method(GET).path("/v1.0/me/events/synthetic-event");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        editable_event_json(
                            "synthetic-version-2",
                            "Concurrent change",
                            "2030-01-08T09:00:00.0000000",
                            "2030-01-08T10:00:00.0000000",
                        )
                        .to_string(),
                    );
            })
            .await;
        let patch = server
            .mock_async(|when, then| {
                when.method(PATCH).path("/v1.0/me/events/synthetic-event");
                then.status(200).body("{}");
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);

        assert_eq!(
            api.update_calendar_event(&update, synthetic_access_token())
                .await,
            Err(GraphError::CalendarEventChanged)
        );
        read.assert_calls_async(1).await;
        patch.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn event_update_refuses_a_missing_event_without_patching() {
        let server = MockServer::start_async().await;
        let update = CalendarEventUpdate::new(
            editable_event("synthetic-version-1"),
            CalendarEventUpdateDraft::timed(
                "Updated planning".to_owned(),
                utc_time(8, 11),
                utc_time(8, 12),
            )
            .expect("synthetic update draft is valid"),
        )
        .expect("synthetic event values changed");
        let read = server
            .mock_async(|when, then| {
                when.method(GET).path("/v1.0/me/events/synthetic-event");
                then.status(404);
            })
            .await;
        let patch = server
            .mock_async(|when, then| {
                when.method(PATCH).path("/v1.0/me/events/synthetic-event");
                then.status(200).body("{}");
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);

        assert_eq!(
            api.update_calendar_event(&update, synthetic_access_token())
                .await,
            Err(GraphError::CalendarEventChanged)
        );
        read.assert_calls_async(1).await;
        patch.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn event_delete_rechecks_version_and_sends_the_current_etag() {
        let server = MockServer::start_async().await;
        let event = editable_event("synthetic-version-1");
        let read = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/events/synthetic-event")
                    .header("prefer", "IdType=\"ImmutableId\"")
                    .query_param("$select", CALENDAR_EVENT_EDIT_SELECT);
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        editable_event_json(
                            "synthetic-version-1",
                            "Planning",
                            "2030-01-08T09:00:00.0000000",
                            "2030-01-08T10:00:00.0000000",
                        )
                        .to_string(),
                    );
            })
            .await;
        let delete = server
            .mock_async(|when, then| {
                when.method(DELETE)
                    .path("/v1.0/me/events/synthetic-event")
                    .header("prefer", "IdType=\"ImmutableId\"")
                    .header("if-match", "W/\"synthetic-etag-1\"");
                then.status(204);
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);

        api.delete_calendar_event(&event, synthetic_access_token())
            .await
            .expect("Graph confirms the conditional delete");

        read.assert_calls_async(1).await;
        delete.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn event_delete_refuses_a_changed_version_without_deleting() {
        let server = MockServer::start_async().await;
        let event = editable_event("synthetic-version-1");
        let read = server
            .mock_async(|when, then| {
                when.method(GET).path("/v1.0/me/events/synthetic-event");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        editable_event_json(
                            "synthetic-version-2",
                            "Changed planning",
                            "2030-01-08T09:00:00.0000000",
                            "2030-01-08T10:00:00.0000000",
                        )
                        .to_string(),
                    );
            })
            .await;
        let delete = server
            .mock_async(|when, then| {
                when.method(DELETE).path("/v1.0/me/events/synthetic-event");
                then.status(204);
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);

        assert_eq!(
            api.delete_calendar_event(&event, synthetic_access_token())
                .await,
            Err(GraphError::CalendarEventChanged)
        );
        read.assert_calls_async(1).await;
        delete.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn event_delete_requires_an_etag_before_sending() {
        let server = MockServer::start_async().await;
        let event = editable_event("synthetic-version-1");
        let mut current = editable_event_json(
            "synthetic-version-1",
            "Planning",
            "2030-01-08T09:00:00.0000000",
            "2030-01-08T10:00:00.0000000",
        );
        current
            .as_object_mut()
            .expect("synthetic event JSON is an object")
            .remove("@odata.etag");
        let read = server
            .mock_async(|when, then| {
                when.method(GET).path("/v1.0/me/events/synthetic-event");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(current.to_string());
            })
            .await;
        let delete = server
            .mock_async(|when, then| {
                when.method(DELETE).path("/v1.0/me/events/synthetic-event");
                then.status(204);
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);

        assert_eq!(
            api.delete_calendar_event(&event, synthetic_access_token())
                .await,
            Err(GraphError::InvalidResponse)
        );
        read.assert_calls_async(1).await;
        delete.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn calendar_delta_uses_exact_week_bounds_and_maps_sparse_followup_changes() {
        let server = MockServer::start_async().await;
        let next_link = server.url("/v1.0/me/calendarView/delta?$skiptoken=synthetic-next");
        let final_link = server.url("/v1.0/me/calendarView/delta?$deltatoken=synthetic-final");
        let initial_page = json!({
            "value": [{
                "id": "synthetic-event",
                "subject": "Synthetic all-day appointment",
                "location": {"displayName": "Room 4"},
                "start": {
                    "dateTime": "2030-01-07T00:00:00.0000000",
                    "timeZone": "UTC"
                },
                "end": {
                    "dateTime": "2030-01-08T00:00:00.0000000",
                    "timeZone": "UTC"
                },
                "isAllDay": true,
                "originalStartTimeZone": "Pacific Standard Time",
                "originalEndTimeZone": "Pacific Standard Time"
            }],
            "@odata.nextLink": next_link.clone()
        });
        let continuation_page = json!({
            "value": [{
                "id": "synthetic-event",
                "location": null
            }],
            "@odata.deltaLink": final_link.clone()
        });
        let first_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/calendarView/delta")
                    .query_param("startDateTime", "2030-01-07T00:00:00Z")
                    .query_param("endDateTime", "2030-01-14T00:00:00Z")
                    .header("content-type", "application/json")
                    .header("prefer", "IdType=\"ImmutableId\"");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(initial_page.to_string());
            })
            .await;
        let continuation_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/calendarView/delta")
                    .query_param("$skiptoken", "synthetic-next")
                    .header("content-type", "application/json")
                    .header("prefer", "IdType=\"ImmutableId\"");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(continuation_page.to_string());
            })
            .await;
        let (api, _transport) = synthetic_graph_api(&server);
        let view = synthetic_view();
        let token = synthetic_access_token();

        let initial = api
            .calendar_delta_page(&view, CalendarDeltaRequest::Initial, Arc::clone(&token))
            .await
            .expect("synthetic bounded calendar delta request succeeds");
        assert!(initial.response_bytes() > 0);
        let (initial_page, _) = initial.into_parts();
        let (mut changes, progress) = initial_page.into_parts();
        let next = match progress {
            CalendarDeltaProgress::Next(link) => link,
            CalendarDeltaProgress::Complete(_) => panic!("initial response must have a next link"),
        };
        assert_eq!(next.as_str(), next_link);
        let CalendarDeltaChange::Upsert { id, patch } = changes.remove(0) else {
            panic!("synthetic event must be an upsert");
        };
        let mut occurrence: CalendarOccurrence = patch
            .into_new(view.clone(), id)
            .expect("new Graph occurrence includes a complete date range");
        assert_eq!(occurrence.subject(), Some("Synthetic all-day appointment"));
        assert_eq!(occurrence.location(), Some("Room 4"));
        assert_eq!(
            occurrence.original_start_time_zone(),
            Some("Pacific Standard Time")
        );
        assert!(matches!(
            occurrence.time(),
            CalendarOccurrenceTime::AllDay { start, end_exclusive }
                if *start == Date::from_calendar_date(2030, Month::January, 7).expect("valid date")
                    && *end_exclusive == Date::from_calendar_date(2030, Month::January, 8).expect("valid date")
        ));

        let continuation = api
            .calendar_delta_page(&view, CalendarDeltaRequest::Continue(next), token)
            .await
            .expect("synthetic continuation request succeeds");
        let (continuation_page, _) = continuation.into_parts();
        let (mut changes, progress) = continuation_page.into_parts();
        assert!(
            matches!(progress, CalendarDeltaProgress::Complete(ref link) if link.as_str() == final_link)
        );
        let CalendarDeltaChange::Upsert { patch, .. } = changes.remove(0) else {
            panic!("synthetic sparse update must be an upsert");
        };
        patch
            .apply_to(&mut occurrence)
            .expect("sparse update preserves the existing occurrence range");
        assert_eq!(occurrence.subject(), Some("Synthetic all-day appointment"));
        assert_eq!(occurrence.location(), None);
        assert!(matches!(
            occurrence.time(),
            CalendarOccurrenceTime::AllDay { .. }
        ));

        first_request.assert_async().await;
        continuation_request.assert_async().await;
    }

    #[test]
    fn rejects_non_utc_and_explicit_offset_calendar_times() {
        for (date_time, time_zone) in [
            ("2030-01-07T09:00:00.0000000", "Pacific Standard Time"),
            ("2030-01-07T09:00:00+01:00", "UTC"),
        ] {
            let event: WireCalendarEvent = serde_json::from_value(json!({
                "id": "synthetic-event",
                "start": {"dateTime": date_time, "timeZone": time_zone}
            }))
            .expect("synthetic event wire fields are valid JSON");
            assert_eq!(event.into_change(), Err(GraphError::InvalidRecord));
        }
    }

    #[test]
    fn created_event_response_requires_a_complete_timed_range() {
        let event: WireCalendarEvent = serde_json::from_value(json!({
            "id": "synthetic-event",
            "subject": "Synthetic planning meeting",
            "isAllDay": false
        }))
        .expect("synthetic incomplete event is valid JSON");

        assert_eq!(
            event.into_new(synthetic_view()),
            Err(GraphError::InvalidRecord)
        );
    }

    #[test]
    fn pagination_budget_accepts_limits_and_rejects_additional_work() {
        let mut budget = PaginationBudget::default();
        for _ in 0..MAX_PAGINATION_PAGES {
            budget
                .begin_page()
                .expect("page is within the configured limit");
        }
        assert_eq!(
            budget.begin_page(),
            Err(GraphError::PaginationLimitExceeded)
        );

        let mut byte_budget = PaginationBudget::default();
        byte_budget
            .add_response_bytes(MAX_PAGINATION_RESPONSE_BYTES)
            .expect("response bytes are within the configured limit");
        assert_eq!(
            byte_budget.add_response_bytes(1),
            Err(GraphError::PaginationLimitExceeded)
        );
    }

    #[test]
    fn missing_hidden_folder_flag_defaults_to_visible() {
        let folder: WireFolder = serde_json::from_value(json!({
            "id": "folder-1",
            "displayName": "Inbox"
        }))
        .expect("synthetic folder without isHidden is valid");

        assert!(!folder.is_hidden);
    }

    #[test]
    fn maps_folder_and_message_payloads_to_account_scoped_domain_values() {
        let account_id = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is non-empty");
        let folder = WireFolder {
            id: "folder-1".to_owned(),
            display_name: "Inbox".to_owned(),
            is_hidden: false,
        }
        .into_domain(&account_id, None)
        .expect("synthetic folder maps to the domain")
        .expect("synthetic folder is visible");
        let message: WireMessage = serde_json::from_value(json!({
            "id": "message-1",
            "subject": "Synthetic subject",
            "from": {"emailAddress": {"address": "sender@example.test", "name": "Sender"}},
            "sender": null,
            "receivedDateTime": "2026-10-30T12:00:00Z",
            "isRead": false
        }))
        .expect("synthetic message payload is valid");
        let message = message
            .into_domain(folder.reference().clone())
            .expect("synthetic message maps to the domain");

        assert_eq!(message.folder().account_id(), &account_id);
        assert_eq!(message.metadata().subject(), Some("Synthetic subject"));
        assert_eq!(
            message
                .metadata()
                .origin()
                .from_mailbox()
                .map(|mailbox| mailbox.address()),
            Some("sender@example.test")
        );
        assert!(message.metadata().origin().sender().is_none());
        assert_eq!(
            message.metadata().read_state(),
            einfach_mail_domain::ReadState::Unread
        );
    }

    #[test]
    fn rejects_empty_folder_ids_and_malformed_message_times() {
        let account_id = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is non-empty");
        assert!(
            WireFolder {
                id: String::new(),
                display_name: "Inbox".to_owned(),
                is_hidden: false,
            }
            .into_domain(&account_id, None)
            .is_err()
        );

        let message: WireMessage = serde_json::from_value(json!({
            "id": "message-1",
            "receivedDateTime": "not-a-timestamp",
            "isRead": true
        }))
        .expect("synthetic message payload has all required fields");
        let folder_id =
            FolderId::try_from("folder-1".to_owned()).expect("synthetic folder ID is non-empty");
        let folder = FolderReference::new(account_id, folder_id);

        assert_eq!(message.into_domain(folder), Err(GraphError::InvalidRecord));
    }

    #[test]
    fn rejects_folder_metadata_that_lists_itself_as_parent() {
        let account_id = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is non-empty");
        let parent_id =
            FolderId::try_from("folder-1".to_owned()).expect("synthetic folder ID is non-empty");
        let folder = WireFolder {
            id: "folder-1".to_owned(),
            display_name: "Child".to_owned(),
            is_hidden: false,
        }
        .into_domain(&account_id, Some(parent_id));

        assert_eq!(folder.err(), Some(GraphError::InvalidRecord));
    }

    #[test]
    fn excludes_hidden_folders_from_the_browsable_tree() {
        let account_id = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is non-empty");
        let folder = WireFolder {
            id: "folder-hidden".to_owned(),
            display_name: "Hidden".to_owned(),
            is_hidden: true,
        }
        .into_domain(&account_id, None)
        .expect("hidden folder metadata is valid");

        assert!(folder.is_none());
    }
}
