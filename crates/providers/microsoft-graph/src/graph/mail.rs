use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::SystemTime,
};

use huskarl::token::AccessToken;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    error::GraphError,
    transport::{GraphPatchOperation, GraphPostJsonOperation, GraphReadOperation, GraphResponse},
};
use einfach_mail_application::{
    MessageDeltaPageResponse, MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, FolderId, FolderReference, FollowUpFlagStatus, MailDraft, MailFolder, MailMessage,
    MailReplyDraft, Mailbox, MessageBody, MessageBodyType, MessageDeltaChange, MessageDeltaLink,
    MessageDeltaPage, MessageDeltaProgress, MessageId, MessageMetadata, MessageMetadataPatch,
    MessageOrigin, ReadState,
};

use super::{
    GraphApi,
    pagination::{GraphDeltaPage, PageTraversal},
    shared::{WireField, WireRemoved},
};

impl GraphApi {
    pub(crate) async fn list_folders(
        &self,
        account_id: &AccountId,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<Vec<MailFolder>, GraphError> {
        let mut traversal = PageTraversal::new(self, GraphReadOperation::Folders, token);
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

    pub(crate) async fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
        token: Arc<AccessToken>,
    ) -> Result<FolderReference, GraphError> {
        let response = self
            .transport
            .get(
                GraphReadOperation::Folders,
                self.authority.well_known_folder_path(folder)?,
                token,
            )
            .await?;
        let wire_folder = serde_json::from_slice::<WireFolder>(&response)
            .map_err(|_| GraphError::InvalidResponse)?;
        wire_folder
            .into_domain(account_id, None)?
            .map(|folder| folder.reference().clone())
            .ok_or(GraphError::InvalidResponse)
    }

    pub(crate) async fn list_messages(
        &self,
        folder: &FolderReference,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<Vec<MailMessage>, GraphError> {
        let mut traversal = PageTraversal::new(self, GraphReadOperation::Messages, token);
        let folder = folder.clone();
        traversal
            .collect::<WireMessage, MailMessage>(
                self.authority
                    .folder_messages_path(folder.folder_id().as_str())?,
                |message| message.into_domain(folder.clone()),
            )
            .await
    }

    pub(crate) async fn message_delta_page(
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
            GraphResponse::CalendarViewDeltaReset { .. } => {
                return Err(GraphError::InvalidResponse);
            }
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

    pub(crate) async fn fetch_message_body(
        &self,
        message_id: &MessageId,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<MessageBody, GraphError> {
        let url = self.authority.message_body_path(message_id.as_str())?;
        let response = self
            .transport
            .get(GraphReadOperation::MessageBody, url, token)
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

    pub(crate) async fn set_message_read_state(
        &self,
        message_id: &MessageId,
        requested: ReadState,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<ReadState, GraphError> {
        let response = self
            .transport
            .patch_json(
                GraphPatchOperation::MessageUpdate,
                self.authority.message_update_path(message_id.as_str())?,
                token,
                read_state_patch_body(requested),
            )
            .await?;
        confirmed_read_state(&response, requested)
    }

    pub(crate) async fn set_message_follow_up_flag_status(
        &self,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<FollowUpFlagStatus, GraphError> {
        let response = self
            .transport
            .patch_json(
                GraphPatchOperation::MessageUpdate,
                self.authority.message_update_path(message_id.as_str())?,
                token,
                follow_up_flag_patch_body(status),
            )
            .await?;
        confirmed_follow_up_flag_status(&response, status)
    }

    pub(crate) async fn move_message(
        &self,
        message_id: &MessageId,
        destination_folder: &FolderReference,
        token: Arc<AccessToken>,
    ) -> Result<(), GraphError> {
        let url = self.authority.message_move_path(message_id.as_str())?;
        let response = self
            .transport
            .post_json(
                GraphPostJsonOperation::MessageMove,
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

    pub(crate) async fn send_mail(
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

    pub(crate) async fn reply_to_message(
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

pub(super) fn mail_reply_body(draft: &MailReplyDraft) -> Result<Vec<u8>, GraphError> {
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
pub(super) struct WireFolder {
    pub(super) id: String,
    pub(super) display_name: String,
    #[serde(default)]
    pub(super) is_hidden: bool,
}

impl WireFolder {
    pub(super) fn into_domain(
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
pub(super) struct WireRecipient {
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
pub(super) struct WireMessage {
    pub(super) id: String,
    pub(super) subject: Option<String>,
    pub(super) from: Option<WireRecipient>,
    pub(super) sender: Option<WireRecipient>,
    pub(super) received_date_time: String,
    pub(super) is_read: bool,
    pub(super) flag: Option<WireFollowUpFlag>,
}

impl WireMessage {
    pub(super) fn into_domain(self, folder: FolderReference) -> Result<MailMessage, GraphError> {
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
pub(super) struct WireMessageChange {
    pub(super) id: String,
    #[serde(flatten)]
    pub(super) metadata: WireMessageMetadata,
    #[serde(rename = "@removed")]
    pub(super) removed: Option<WireRemoved>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireMessageMetadata {
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
pub(super) struct WireFollowUpFlag {
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

pub(super) fn read_state_patch_body(read_state: ReadState) -> Arc<[u8]> {
    match read_state {
        ReadState::Read => Arc::from(&b"{\"isRead\":true}"[..]),
        ReadState::Unread => Arc::from(&b"{\"isRead\":false}"[..]),
    }
}

pub(super) fn confirmed_read_state(
    response: &[u8],
    requested: ReadState,
) -> Result<ReadState, GraphError> {
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

pub(super) fn follow_up_flag_patch_body(status: FollowUpFlagStatus) -> Arc<[u8]> {
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

pub(super) fn message_move_body(destination_id: &str) -> Result<Arc<[u8]>, GraphError> {
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

pub(super) fn confirmed_message_move(
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

pub(super) fn confirmed_follow_up_flag_status(
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
