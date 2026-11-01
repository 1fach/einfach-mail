//! Microsoft Graph profile, folder, and message mapping.

use std::{
    collections::{HashSet, VecDeque},
    time::SystemTime,
};

use huskarl::token::AccessToken;
use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

use crate::{
    error::GraphError,
    graph_url::{GraphAuthority, GraphUrl},
    transport::{GraphOperation, GraphTransportHandle},
};
use einfach_mail_domain::{
    AccountId, AccountProfile, FolderId, FolderReference, MailFolder, MailMessage, Mailbox,
    MessageId, MessageMetadata, MessageOrigin, ReadState,
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
            .collect::<WireFolder, MailFolder>(self.authority.root_folders_path()?, |folder| {
                folder.into_domain(account_id, None)
            })
            .await?;
        let mut tree = FolderTree::new();
        tree.add(roots)?;
        while let Some(parent_id) = tree.pending.pop_front() {
            let children = traversal
                .collect::<WireFolder, MailFolder>(
                    self.authority.child_folders_path(parent_id.as_str())?,
                    |folder| folder.into_domain(account_id, Some(parent_id.clone())),
                )
                .await?;
            tree.add(children)?;
        }
        Ok(tree.folders)
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
struct WireFolder {
    id: String,
    display_name: String,
}

impl WireFolder {
    fn into_domain(
        self,
        account_id: &AccountId,
        parent_id: Option<FolderId>,
    ) -> Result<MailFolder, GraphError> {
        let folder_id = FolderId::try_from(self.id).map_err(|_| GraphError::InvalidRecord)?;
        let reference = FolderReference::new(account_id.clone(), folder_id);
        let parent = parent_id.map(|folder_id| FolderReference::new(account_id.clone(), folder_id));
        MailFolder::new(reference, parent, self.display_name).map_err(|_| GraphError::InvalidRecord)
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
        let metadata = MessageMetadata::new(self.subject, origin, received_at, read_state);
        Ok(MailMessage::new(folder, id, metadata))
    }
}

#[cfg(test)]
mod tests {
    use einfach_mail_domain::{AccountId, FolderId, FolderReference};
    use serde_json::json;

    use super::{
        MAX_PAGINATION_PAGES, MAX_PAGINATION_RESPONSE_BYTES, PaginationBudget, WireFolder,
        WireMessage,
    };
    use crate::error::GraphError;

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
    fn maps_folder_and_message_payloads_to_account_scoped_domain_values() {
        let account_id = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is non-empty");
        let folder = WireFolder {
            id: "folder-1".to_owned(),
            display_name: "Inbox".to_owned(),
        }
        .into_domain(&account_id, None)
        .expect("synthetic folder maps to the domain");
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
        }
        .into_domain(&account_id, Some(parent_id));

        assert_eq!(folder.err(), Some(GraphError::InvalidRecord));
    }
}
