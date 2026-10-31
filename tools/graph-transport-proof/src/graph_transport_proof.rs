use async_channel::{Receiver, Sender, bounded};
use einfach_mail_domain::{
    AccountId, EventId, FolderId, FolderReference, MailFolder, MailMessage, Mailbox, MessageId,
    MessageMetadata, MessageMetadataPatch, MessageOrigin, ReadState,
};
use reqwest::{
    Client, Response, StatusCode, Url,
    header::{AUTHORIZATION, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use serde::Deserialize;
use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, SystemTime},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{runtime::Builder, task::JoinSet};

const MAX_THROTTLE_RETRIES: u32 = 2;
const MAX_AUTOMATIC_RETRY_DELAY: Duration = Duration::from_mins(1);
const REQUEST_CHANNEL_CAPACITY: usize = 16;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_PAGINATION_PAGES: usize = 1_000;
const MAX_PAGINATION_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const GRAPH_API_PATH_PREFIX: &str = "/v1.0/";
const FOLDER_SELECT: &str = "id,displayName";
const MESSAGE_SELECT: &str = "id,subject,from,sender,receivedDateTime,isRead";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetryAfter {
    Delay(Duration),
    Unrepresentable,
}

#[derive(Debug, Eq, PartialEq)]
enum ProviderError {
    Startup,
    WorkerClosed,
    Transport,
    InvalidToken,
    Rejected,
    Throttled { retry_after: RetryAfter },
    MalformedResponse,
    InvalidUrl,
    UnsafeLink,
    InvalidRecord,
    ResponseTooLarge,
    PaginationLimitExceeded,
}

#[derive(Clone, Eq, PartialEq)]
struct GraphOrigin {
    scheme: String,
    host: String,
    port: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GraphDeployment {
    Global,
    UsGovernmentL4,
    UsGovernmentL5,
    China21Vianet,
}

impl GraphDeployment {
    const fn service_root(self) -> &'static str {
        match self {
            Self::Global => "https://graph.microsoft.com",
            Self::UsGovernmentL4 => "https://graph.microsoft.us",
            Self::UsGovernmentL5 => "https://dod-graph.microsoft.us",
            Self::China21Vianet => "https://microsoftgraph.chinacloudapi.cn",
        }
    }
}

#[derive(Clone)]
struct GraphAuthority {
    base_url: Url,
    origin: GraphOrigin,
}

impl GraphAuthority {
    fn for_deployment(deployment: GraphDeployment) -> Result<Self, ProviderError> {
        let mut base_url =
            Url::parse(deployment.service_root()).map_err(|_| ProviderError::InvalidUrl)?;
        base_url.set_path("/v1.0/");
        Self::from_base_url(base_url)
    }

    fn for_loopback_mock(base_url: &str) -> Result<Self, ProviderError> {
        if has_user_info_separator(base_url) {
            return Err(ProviderError::InvalidUrl);
        }
        let mut base_url = Url::parse(base_url).map_err(|_| ProviderError::InvalidUrl)?;
        let host = base_url
            .host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .ok_or(ProviderError::InvalidUrl)?;
        if base_url.scheme() != "http" || !host.is_loopback() {
            return Err(ProviderError::InvalidUrl);
        }

        base_url.set_path("/v1.0/");
        base_url.set_query(None);
        base_url.set_fragment(None);
        Self::from_base_url(base_url)
    }

    fn from_base_url(base_url: Url) -> Result<Self, ProviderError> {
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(ProviderError::InvalidUrl);
        }

        let origin = Self::origin_of(&base_url)?;
        Ok(Self { base_url, origin })
    }

    fn initial_path(&self, path_and_query: &str) -> Result<GraphUrl, ProviderError> {
        if path_and_query.starts_with('/') {
            return Err(ProviderError::InvalidUrl);
        }
        let url = self
            .base_url
            .join(path_and_query)
            .map_err(|_| ProviderError::InvalidUrl)?;
        self.validate_url(&url)?;
        Ok(GraphUrl(url.to_string()))
    }

    fn child_folders_path(&self, folder_id: &str) -> Result<GraphUrl, ProviderError> {
        let mut url = self.folder_path(folder_id, &["childFolders"])?;
        url.query_pairs_mut().append_pair("$select", FOLDER_SELECT);
        Ok(GraphUrl(url.to_string()))
    }

    fn folder_messages_path(&self, folder_id: &str) -> Result<GraphUrl, ProviderError> {
        let mut url = self.folder_path(folder_id, &["messages"])?;
        url.query_pairs_mut().append_pair("$select", MESSAGE_SELECT);
        Ok(GraphUrl(url.to_string()))
    }

    fn initial_message_delta_path(
        &self,
        folder_id: &str,
        received_since: &str,
    ) -> Result<GraphUrl, ProviderError> {
        let mut url = self.folder_path(folder_id, &["messages", "delta"])?;
        let filter = format!("receivedDateTime ge {received_since}");
        url.query_pairs_mut()
            .append_pair("$select", MESSAGE_SELECT)
            .append_pair("$filter", &filter);
        Ok(GraphUrl(url.to_string()))
    }

    fn folder_path(&self, folder_id: &str, tail: &[&str]) -> Result<Url, ProviderError> {
        if folder_id.is_empty() {
            return Err(ProviderError::InvalidRecord);
        }

        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|()| ProviderError::InvalidUrl)?;
            segments.pop_if_empty().extend(
                ["me", "mailFolders", folder_id]
                    .into_iter()
                    .chain(tail.iter().copied()),
            );
        }
        self.validate_url(&url)?;
        Ok(url)
    }

    fn opaque_link(&self, link: &str) -> Result<GraphUrl, ProviderError> {
        if has_user_info_separator(link) {
            return Err(ProviderError::UnsafeLink);
        }
        let parsed = Url::parse(link).map_err(|_| ProviderError::UnsafeLink)?;
        self.validate_url(&parsed)?;
        Ok(GraphUrl(link.to_owned()))
    }

    fn validate_url(&self, url: &Url) -> Result<(), ProviderError> {
        if !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !url.path().starts_with(GRAPH_API_PATH_PREFIX)
            || Self::origin_of(url)? != self.origin
        {
            return Err(ProviderError::UnsafeLink);
        }
        Ok(())
    }

    fn origin_of(url: &Url) -> Result<GraphOrigin, ProviderError> {
        Ok(GraphOrigin {
            scheme: url.scheme().to_owned(),
            host: url.host_str().ok_or(ProviderError::InvalidUrl)?.to_owned(),
            port: url
                .port_or_known_default()
                .ok_or(ProviderError::InvalidUrl)?,
        })
    }
}

fn has_user_info_separator(raw_url: &str) -> bool {
    raw_url
        .split_once("://")
        .is_some_and(|(_, authority_and_rest)| {
            authority_and_rest
                .split(['/', '?', '#'])
                .next()
                .is_some_and(|authority| authority.contains('@'))
        })
}

struct GraphUrl(String);

impl GraphUrl {
    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone)]
struct BearerToken(Arc<str>);

impl BearerToken {
    fn synthetic() -> Self {
        Self(Arc::from("synthetic-test-token"))
    }
}

#[derive(Clone, Copy)]
enum GraphOperation {
    Folders,
    Messages,
    MessageDelta,
    CalendarViewDelta,
}

impl GraphOperation {
    fn uses_immutable_ids(self) -> bool {
        matches!(
            self,
            Self::Messages | Self::MessageDelta | Self::CalendarViewDelta
        )
    }
}

struct GetCommand {
    operation: GraphOperation,
    url: GraphUrl,
    token: BearerToken,
    reply: Sender<Result<Vec<u8>, ProviderError>>,
}

struct PendingResponse(Receiver<Result<Vec<u8>, ProviderError>>);

impl PendingResponse {
    async fn receive(self) -> Result<Vec<u8>, ProviderError> {
        self.0
            .recv()
            .await
            .map_err(|_| ProviderError::WorkerClosed)?
    }
}

#[derive(Default)]
struct RequestTracker(AtomicUsize);

impl RequestTracker {
    fn begin(self: &Arc<Self>) -> ActiveRequest {
        self.0.fetch_add(1, Ordering::SeqCst);
        ActiveRequest(Arc::clone(self))
    }

    fn active(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

struct ActiveRequest(Arc<RequestTracker>);

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.0.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct GraphProvider {
    authority: GraphAuthority,
    commands: Sender<GetCommand>,
    shutdown: Sender<()>,
    stopped: Receiver<()>,
    _worker: JoinHandle<()>,
    requests: Arc<RequestTracker>,
}

impl GraphProvider {
    async fn start(authority: GraphAuthority) -> Result<Self, ProviderError> {
        let (commands, command_rx) = bounded(REQUEST_CHANNEL_CAPACITY);
        let (shutdown, shutdown_rx) = bounded(1);
        let (ready_tx, ready_rx) = bounded(1);
        let (stopped_tx, stopped) = bounded(1);
        let requests = Arc::new(RequestTracker::default());
        let worker_requests = Arc::clone(&requests);

        let worker = std::thread::Builder::new()
            .name("einfach-mail-graph-transport-proof".to_owned())
            .spawn(move || {
                let Ok(runtime) = Builder::new_current_thread().enable_all().build() else {
                    let _ = ready_tx.send_blocking(Err(ProviderError::Startup));
                    let _ = stopped_tx.send_blocking(());
                    return;
                };

                let Ok(client) = build_http_client(REQUEST_TIMEOUT) else {
                    let _ = ready_tx.send_blocking(Err(ProviderError::Startup));
                    drop(runtime);
                    let _ = stopped_tx.send_blocking(());
                    return;
                };

                if ready_tx.send_blocking(Ok(())).is_err() {
                    drop(runtime);
                    let _ = stopped_tx.send_blocking(());
                    return;
                }

                runtime.block_on(run_worker(command_rx, shutdown_rx, client, worker_requests));
                drop(runtime);
                let _ = stopped_tx.send_blocking(());
            })
            .map_err(|_| ProviderError::Startup)?;

        match ready_rx.recv().await {
            Ok(Ok(())) => Ok(Self {
                authority,
                commands,
                shutdown,
                stopped,
                _worker: worker,
                requests,
            }),
            Ok(Err(error)) => {
                join_worker(worker).await?;
                Err(error)
            }
            Err(_) => {
                commands.close();
                shutdown.close();
                join_worker(worker).await?;
                Err(ProviderError::WorkerClosed)
            }
        }
    }

    async fn begin_get(
        &self,
        operation: GraphOperation,
        url: GraphUrl,
        token: BearerToken,
    ) -> Result<PendingResponse, ProviderError> {
        let (reply, response) = bounded(1);
        self.commands
            .send(GetCommand {
                operation,
                url,
                token,
                reply,
            })
            .await
            .map_err(|_| ProviderError::WorkerClosed)?;
        Ok(PendingResponse(response))
    }

    async fn get(
        &self,
        operation: GraphOperation,
        url: GraphUrl,
        token: &BearerToken,
    ) -> Result<Vec<u8>, ProviderError> {
        self.begin_get(operation, url, token.clone())
            .await?
            .receive()
            .await
    }

    async fn folders(
        &self,
        account_id: &AccountId,
        token: &BearerToken,
    ) -> Result<Vec<MailFolder>, ProviderError> {
        let mut budget = PaginationBudget::default();
        let roots = self.root_folders(account_id, token, &mut budget).await?;
        self.collect_folder_tree(account_id, roots, token, &mut budget)
            .await
    }

    async fn root_folders(
        &self,
        account_id: &AccountId,
        token: &BearerToken,
        budget: &mut PaginationBudget,
    ) -> Result<Vec<MailFolder>, ProviderError> {
        let path = self
            .authority
            .initial_path(&format!("me/mailFolders?$select={FOLDER_SELECT}"))?;
        self.folder_pages(path, account_id, None, token, budget)
            .await
    }

    async fn child_folders(
        &self,
        account_id: &AccountId,
        parent_id: &FolderId,
        token: &BearerToken,
        budget: &mut PaginationBudget,
    ) -> Result<Vec<MailFolder>, ProviderError> {
        let path = self.authority.child_folders_path(parent_id.as_str())?;
        self.folder_pages(path, account_id, Some(parent_id.clone()), token, budget)
            .await
    }

    async fn folder_pages(
        &self,
        start: GraphUrl,
        account_id: &AccountId,
        parent_id: Option<FolderId>,
        token: &BearerToken,
        budget: &mut PaginationBudget,
    ) -> Result<Vec<MailFolder>, ProviderError> {
        self.list_pages::<WireFolder, MailFolder>(
            start,
            GraphOperation::Folders,
            token,
            budget,
            |folder| folder.into_domain(account_id, parent_id.clone()),
        )
        .await
    }

    async fn collect_folder_tree(
        &self,
        account_id: &AccountId,
        roots: Vec<MailFolder>,
        token: &BearerToken,
        budget: &mut PaginationBudget,
    ) -> Result<Vec<MailFolder>, ProviderError> {
        let mut folders = Vec::new();
        let mut pending = VecDeque::new();
        let mut seen = HashSet::new();
        Self::queue_folders(&mut folders, &mut pending, &mut seen, roots)?;

        while let Some(parent_id) = pending.pop_front() {
            let children = self
                .child_folders(account_id, &parent_id, token, budget)
                .await?;
            Self::queue_folders(&mut folders, &mut pending, &mut seen, children)?;
        }

        Ok(folders)
    }

    fn queue_folders(
        folders: &mut Vec<MailFolder>,
        pending: &mut VecDeque<FolderId>,
        seen: &mut HashSet<FolderId>,
        next: Vec<MailFolder>,
    ) -> Result<(), ProviderError> {
        for folder in next {
            let folder_id = folder.reference().folder_id().clone();
            if !seen.insert(folder_id.clone()) {
                return Err(ProviderError::InvalidRecord);
            }
            pending.push_back(folder_id);
            folders.push(folder);
        }
        Ok(())
    }

    async fn messages(
        &self,
        folder: &FolderReference,
        token: &BearerToken,
    ) -> Result<Vec<MailMessage>, ProviderError> {
        let mut budget = PaginationBudget::default();
        let folder = folder.clone();
        self.list_pages::<WireMessage, MailMessage>(
            self.authority
                .folder_messages_path(folder.folder_id().as_str())?,
            GraphOperation::Messages,
            token,
            &mut budget,
            |message| message.into_domain(folder.clone()),
        )
        .await
    }

    async fn list_pages<T, U>(
        &self,
        start: GraphUrl,
        operation: GraphOperation,
        token: &BearerToken,
        budget: &mut PaginationBudget,
        mut convert: impl FnMut(T) -> Result<U, ProviderError>,
    ) -> Result<Vec<U>, ProviderError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut next = start;
        let mut seen = HashSet::new();
        let mut records = Vec::new();

        loop {
            if !seen.insert(next.as_str().to_owned()) {
                return Err(ProviderError::UnsafeLink);
            }

            let page = self.get_page::<T>(operation, next, token, budget).await?;
            for record in page.value {
                records.push(convert(record)?);
            }

            match page.next_link {
                Some(link) => next = self.authority.opaque_link(&link)?,
                None => return Ok(records),
            }
        }
    }

    async fn message_delta(
        &self,
        start: GraphUrl,
        token: &BearerToken,
    ) -> Result<DeltaRound<MessageChange>, ProviderError> {
        self.delta_pages::<WireMessageChange>(GraphOperation::MessageDelta, start, token)
            .await?
            .map_records(MessageChange::try_from)
    }

    async fn calendar_view_delta(
        &self,
        start: GraphUrl,
        token: &BearerToken,
    ) -> Result<DeltaRound<CalendarChange>, ProviderError> {
        self.delta_pages::<WireCalendarEvent>(GraphOperation::CalendarViewDelta, start, token)
            .await?
            .map_records(CalendarChange::try_from)
    }

    async fn get_page<T: for<'de> Deserialize<'de>>(
        &self,
        operation: GraphOperation,
        url: GraphUrl,
        token: &BearerToken,
        budget: &mut PaginationBudget,
    ) -> Result<GraphPage<T>, ProviderError> {
        budget.begin_page()?;
        let body = self.get(operation, url, token).await?;
        budget.add_response_bytes(body.len())?;
        parse_page(&body)
    }

    async fn delta_pages<T: for<'de> Deserialize<'de>>(
        &self,
        operation: GraphOperation,
        start: GraphUrl,
        token: &BearerToken,
    ) -> Result<DeltaRound<T>, ProviderError> {
        let mut next = start;
        let mut seen = HashSet::new();
        let mut records = Vec::new();
        let mut budget = PaginationBudget::default();

        loop {
            if !seen.insert(next.as_str().to_owned()) {
                return Err(ProviderError::UnsafeLink);
            }
            let page = self.get_page(operation, next, token, &mut budget).await?;
            records.extend(page.value);
            match delta_cursor(&self.authority, page.next_link, page.delta_link)? {
                DeltaCursor::Next(url) => next = url,
                DeltaCursor::Finished(delta_link) => {
                    return Ok(DeltaRound {
                        records,
                        delta_link,
                    });
                }
            }
        }
    }

    fn active_requests(&self) -> usize {
        self.requests.active()
    }

    async fn shutdown(self) -> Result<(), ProviderError> {
        let GraphProvider {
            shutdown,
            stopped,
            _worker: worker,
            ..
        } = self;
        shutdown.close();
        stopped
            .recv()
            .await
            .map_err(|_| ProviderError::WorkerClosed)?;
        join_worker(worker).await
    }
}

async fn join_worker(worker: JoinHandle<()>) -> Result<(), ProviderError> {
    smol::unblock(move || worker.join())
        .await
        .map_err(|_| ProviderError::WorkerClosed)
}

#[derive(Deserialize)]
struct GraphPage<T> {
    value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    delta_link: Option<String>,
}

#[derive(Default)]
struct PaginationBudget {
    pages: usize,
    response_bytes: usize,
}

impl PaginationBudget {
    fn begin_page(&mut self) -> Result<(), ProviderError> {
        if self.pages >= MAX_PAGINATION_PAGES {
            return Err(ProviderError::PaginationLimitExceeded);
        }
        self.pages += 1;
        Ok(())
    }

    fn add_response_bytes(&mut self, bytes: usize) -> Result<(), ProviderError> {
        let total_bytes = self
            .response_bytes
            .checked_add(bytes)
            .ok_or(ProviderError::PaginationLimitExceeded)?;
        if total_bytes > MAX_PAGINATION_RESPONSE_BYTES {
            return Err(ProviderError::PaginationLimitExceeded);
        }
        self.response_bytes = total_bytes;
        Ok(())
    }
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
    ) -> Result<MailFolder, ProviderError> {
        let folder_id = FolderId::try_from(self.id).map_err(|_| ProviderError::InvalidRecord)?;
        let reference = FolderReference::new(account_id.clone(), folder_id);
        let parent = parent_id.map(|folder_id| FolderReference::new(account_id.clone(), folder_id));

        MailFolder::new(reference, parent, self.display_name)
            .map_err(|_| ProviderError::InvalidRecord)
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
    type Error = ProviderError;

    fn try_from(recipient: WireRecipient) -> Result<Self, Self::Error> {
        Mailbox::try_new(
            recipient.email_address.address,
            recipient.email_address.name,
        )
        .map_err(|_| ProviderError::InvalidRecord)
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
    fn into_domain(self, folder: FolderReference) -> Result<MailMessage, ProviderError> {
        let id = MessageId::try_from(self.id).map_err(|_| ProviderError::InvalidRecord)?;
        let received_at = parse_received_at(&self.received_date_time)?;
        let from = self.from.map(Mailbox::try_from).transpose()?;
        let sender = self.sender.map(Mailbox::try_from).transpose()?;
        let origin = MessageOrigin::new(from, sender);
        let read_state = if self.is_read {
            ReadState::Read
        } else {
            ReadState::Unread
        };
        let metadata = MessageMetadata::new(self.subject, origin, received_at, read_state);

        Ok(MailMessage::new(folder, id, metadata))
    }
}

fn parse_received_at(value: &str) -> Result<SystemTime, ProviderError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map(SystemTime::from)
        .map_err(|_| ProviderError::InvalidRecord)
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
}

#[derive(Debug, Default, Eq, PartialEq)]
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

impl TryFrom<WireMessageMetadata> for MessageMetadataPatch {
    type Error = ProviderError;

    fn try_from(metadata: WireMessageMetadata) -> Result<Self, Self::Error> {
        let WireMessageMetadata {
            subject,
            from,
            sender,
            received_date_time,
            is_read,
        } = metadata;
        let mut patch = Self::new();
        apply_nullable_fields(subject, from, sender, &mut patch)?;
        apply_required_fields(received_date_time, &is_read, &mut patch)?;
        Ok(patch)
    }
}

fn apply_nullable_fields(
    subject: WireField<String>,
    from: WireField<WireRecipient>,
    sender: WireField<WireRecipient>,
    patch: &mut MessageMetadataPatch,
) -> Result<(), ProviderError> {
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
    is_read: &WireField<bool>,
    patch: &mut MessageMetadataPatch,
) -> Result<(), ProviderError> {
    match received_date_time {
        WireField::Missing => {}
        WireField::Null => return Err(ProviderError::InvalidRecord),
        WireField::Value(received_date_time) => {
            patch.set_received_at(parse_received_at(&received_date_time)?);
        }
    }
    match is_read {
        WireField::Missing => {}
        WireField::Null => return Err(ProviderError::InvalidRecord),
        WireField::Value(true) => patch.set_read_state(ReadState::Read),
        WireField::Value(false) => patch.set_read_state(ReadState::Unread),
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum MessageChange {
    Upsert {
        id: MessageId,
        patch: MessageMetadataPatch,
    },
    Removed {
        id: MessageId,
        reason: Option<String>,
    },
}

impl TryFrom<WireMessageChange> for MessageChange {
    type Error = ProviderError;

    fn try_from(message: WireMessageChange) -> Result<Self, Self::Error> {
        let WireMessageChange {
            id,
            metadata,
            removed,
        } = message;
        let id = MessageId::try_from(id).map_err(|_| ProviderError::InvalidRecord)?;
        match removed {
            Some(removed) => Ok(Self::Removed {
                id,
                reason: removed.reason,
            }),
            None => Ok(Self::Upsert {
                id,
                patch: metadata.try_into()?,
            }),
        }
    }
}

#[derive(Deserialize)]
struct WireCalendarEvent {
    id: String,
    subject: Option<String>,
    #[serde(rename = "@removed")]
    removed: Option<WireRemoved>,
}

#[derive(Deserialize)]
struct WireRemoved {
    reason: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
enum CalendarChange {
    Upsert {
        id: EventId,
        subject: Option<String>,
    },
    Removed {
        id: EventId,
        reason: Option<String>,
    },
}

impl TryFrom<WireCalendarEvent> for CalendarChange {
    type Error = ProviderError;

    fn try_from(event: WireCalendarEvent) -> Result<Self, Self::Error> {
        let WireCalendarEvent {
            id,
            subject,
            removed,
        } = event;
        let id = EventId::try_from(id).map_err(|_| ProviderError::InvalidRecord)?;
        match removed {
            Some(removed) => Ok(Self::Removed {
                id,
                reason: removed.reason,
            }),
            None => Ok(Self::Upsert { id, subject }),
        }
    }
}

struct DeltaRound<T> {
    records: Vec<T>,
    delta_link: GraphUrl,
}

impl<T> DeltaRound<T> {
    fn map_records<U>(
        self,
        convert: impl FnMut(T) -> Result<U, ProviderError>,
    ) -> Result<DeltaRound<U>, ProviderError> {
        Ok(DeltaRound {
            records: self
                .records
                .into_iter()
                .map(convert)
                .collect::<Result<_, _>>()?,
            delta_link: self.delta_link,
        })
    }
}

enum DeltaCursor {
    Next(GraphUrl),
    Finished(GraphUrl),
}

fn delta_cursor(
    authority: &GraphAuthority,
    next_link: Option<String>,
    delta_link: Option<String>,
) -> Result<DeltaCursor, ProviderError> {
    match (next_link, delta_link) {
        (Some(link), None) => Ok(DeltaCursor::Next(authority.opaque_link(&link)?)),
        (None, Some(link)) => Ok(DeltaCursor::Finished(authority.opaque_link(&link)?)),
        _ => Err(ProviderError::MalformedResponse),
    }
}

fn parse_page<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<GraphPage<T>, ProviderError> {
    serde_json::from_slice(bytes).map_err(|_| ProviderError::MalformedResponse)
}

fn authorization_header(token: &BearerToken) -> Result<HeaderValue, ProviderError> {
    let value = format!("Bearer {}", token.0);
    let mut header =
        HeaderValue::from_bytes(value.as_bytes()).map_err(|_| ProviderError::InvalidToken)?;
    header.set_sensitive(true);
    Ok(header)
}

fn build_http_client(request_timeout: Duration) -> Result<Client, ProviderError> {
    Client::builder()
        // The proof uses a local mock server; production must retain system proxy support.
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| ProviderError::Startup)
}

async fn run_worker(
    commands: Receiver<GetCommand>,
    shutdown: Receiver<()>,
    client: Client,
    requests: Arc<RequestTracker>,
) {
    let mut tasks = JoinSet::new();

    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                tasks.shutdown().await;
                return;
            }
            command = commands.recv(), if tasks.len() < REQUEST_CHANNEL_CAPACITY => {
                let Ok(command) = command else {
                    tasks.shutdown().await;
                    return;
                };
                let client = client.clone();
                let requests = Arc::clone(&requests);
                tasks.spawn(async move {
                    let _active_request = requests.begin();
                    let reply = command.reply.clone();
                    if let Some(result) = request_with_retry(&client, command).await {
                        let _ = reply.send(result).await;
                    }
                });
            }
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if matches!(completed, Some(Err(_))) {
                    tasks.shutdown().await;
                    return;
                }
            }
        }
    }
}

async fn request_with_retry(
    client: &Client,
    command: GetCommand,
) -> Option<Result<Vec<u8>, ProviderError>> {
    let mut retry_index = 0;
    loop {
        let response = match send_once(client, &command).await? {
            Ok(response) => response,
            Err(error) => return Some(Err(error)),
        };

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = retry_after_delay(
                response.headers().get(RETRY_AFTER),
                SystemTime::now(),
                retry_index,
            );
            let delay = match automatic_retry_delay(retry_after, retry_index) {
                Ok(delay) => delay,
                Err(error) => return Some(Err(error)),
            };
            while_interested(&command.reply, tokio::time::sleep(delay)).await?;
            retry_index += 1;
            continue;
        }

        if !response.status().is_success() {
            return Some(Err(ProviderError::Rejected));
        }

        return read_limited_body(response, &command.reply).await;
    }
}

fn automatic_retry_delay(
    retry_after: RetryAfter,
    retry_index: u32,
) -> Result<Duration, ProviderError> {
    match retry_after {
        RetryAfter::Delay(delay)
            if delay <= MAX_AUTOMATIC_RETRY_DELAY && retry_index < MAX_THROTTLE_RETRIES =>
        {
            Ok(delay)
        }
        retry_after => Err(ProviderError::Throttled { retry_after }),
    }
}

fn response_body_is_too_large(response: &Response) -> bool {
    response.content_length().is_some_and(|length| {
        usize::try_from(length).map_or(true, |length| length > MAX_RESPONSE_BODY_BYTES)
    })
}

fn append_response_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), ProviderError> {
    if chunk.len() > MAX_RESPONSE_BODY_BYTES - body.len() {
        return Err(ProviderError::ResponseTooLarge);
    }
    body.extend_from_slice(chunk);
    Ok(())
}

async fn read_limited_body(
    mut response: Response,
    reply: &Sender<Result<Vec<u8>, ProviderError>>,
) -> Option<Result<Vec<u8>, ProviderError>> {
    if response_body_is_too_large(&response) {
        return Some(Err(ProviderError::ResponseTooLarge));
    }

    let mut body = Vec::new();
    loop {
        match while_interested(reply, response.chunk()).await? {
            Ok(Some(chunk)) => match append_response_chunk(&mut body, &chunk) {
                Ok(()) => {}
                Err(error) => return Some(Err(error)),
            },
            Ok(None) => return Some(Ok(body)),
            Err(_) => return Some(Err(ProviderError::Transport)),
        }
    }
}

async fn send_once(
    client: &Client,
    command: &GetCommand,
) -> Option<Result<Response, ProviderError>> {
    let authorization = match authorization_header(&command.token) {
        Ok(header) => header,
        Err(error) => return Some(Err(error)),
    };
    let mut request = client
        .get(command.url.as_str())
        .header(AUTHORIZATION, authorization);
    if command.operation.uses_immutable_ids() {
        request = request.header("Prefer", "IdType=\"ImmutableId\"");
    }

    while_interested(&command.reply, request.send())
        .await?
        .map_err(|_| ProviderError::Transport)
        .into()
}

async fn while_interested<T, F>(
    reply: &Sender<Result<Vec<u8>, ProviderError>>,
    work: F,
) -> Option<T>
where
    F: Future<Output = T>,
{
    tokio::select! {
        biased;
        () = reply.closed() => None,
        result = work => Some(result),
    }
}

fn retry_after_delay(value: Option<&HeaderValue>, now: SystemTime, attempt: u32) -> RetryAfter {
    let Some(value) = value.and_then(|value| value.to_str().ok()) else {
        return RetryAfter::Delay(fallback_delay(attempt));
    };

    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return match value.parse::<u64>() {
            Ok(seconds) => RetryAfter::Delay(Duration::from_secs(seconds)),
            Err(_) => RetryAfter::Unrepresentable,
        };
    }

    match httpdate::parse_http_date(value) {
        Ok(retry_at) => RetryAfter::Delay(retry_at.duration_since(now).unwrap_or(Duration::ZERO)),
        Err(_) => RetryAfter::Delay(fallback_delay(attempt)),
    }
}

fn fallback_delay(attempt: u32) -> Duration {
    let seconds = 2_u64
        .saturating_pow(attempt)
        .min(MAX_AUTOMATIC_RETRY_DELAY.as_secs());
    Duration::from_secs(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{HttpMockRequest, HttpMockResponse, MockServer};

    fn loopback_authority(server: &MockServer) -> GraphAuthority {
        GraphAuthority::for_loopback_mock(&server.base_url())
            .expect("mock server binds to a loopback address")
    }

    fn synthetic_account_id() -> AccountId {
        AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is non-empty")
    }

    fn synthetic_message_id(id: &str) -> MessageId {
        MessageId::try_from(id.to_owned()).expect("synthetic message ID is non-empty")
    }

    fn synthetic_event_id(id: &str) -> EventId {
        EventId::try_from(id.to_owned()).expect("synthetic event ID is non-empty")
    }

    fn synthetic_folder_reference(folder_id: &str) -> FolderReference {
        let folder_id =
            FolderId::try_from(folder_id.to_owned()).expect("synthetic folder ID is non-empty");
        FolderReference::new(synthetic_account_id(), folder_id)
    }

    fn synthetic_received_at() -> SystemTime {
        OffsetDateTime::parse("2026-10-30T12:00:00Z", &Rfc3339)
            .expect("synthetic timestamp is RFC 3339")
            .into()
    }

    fn synthetic_mailbox(address: &str, display_name: Option<&str>) -> Mailbox {
        Mailbox::try_new(address.to_owned(), display_name.map(str::to_owned))
            .expect("synthetic mailbox has a non-empty address")
    }

    fn synthetic_message_metadata() -> MessageMetadata {
        MessageMetadata::new(
            Some("Old subject".to_owned()),
            MessageOrigin::new(
                Some(synthetic_mailbox("old-from@example.test", None)),
                Some(synthetic_mailbox("old-sender@example.test", None)),
            ),
            synthetic_received_at(),
            ReadState::Read,
        )
    }

    fn apply_synthetic_message_delta(metadata: &mut MessageMetadata, payload: serde_json::Value) {
        let wire: WireMessageChange =
            serde_json::from_value(payload).expect("synthetic delta JSON is valid");
        let MessageChange::Upsert { patch, .. } =
            wire.try_into().expect("synthetic delta record is valid")
        else {
            panic!("synthetic record is an upsert");
        };
        metadata.apply_patch(patch);
    }

    async fn mock_message_page<'a>(
        server: &'a MockServer,
        path: &'static str,
        expected_query: Vec<(&'static str, &'static str)>,
        body: String,
    ) -> httpmock::Mock<'a> {
        let expected_query = expected_query
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect::<Vec<_>>();
        server
            .mock_async(move |when, then| {
                when.method("GET")
                    .path(path)
                    .header("Authorization", "Bearer synthetic-test-token")
                    .header("Prefer", "IdType=\"ImmutableId\"")
                    .is_true(move |request| {
                        let actual_query = request.query_params();
                        actual_query.len() == expected_query.len()
                            && expected_query
                                .iter()
                                .all(|expected| actual_query.contains(expected))
                    });
                then.status(200)
                    .header("content-type", "application/json")
                    .body(body);
            })
            .await
    }

    async fn mock_folder_page<'a>(
        server: &'a MockServer,
        path: &'static str,
        expected_query: Vec<(&'static str, &'static str)>,
        body: String,
    ) -> httpmock::Mock<'a> {
        let expected_query = expected_query
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect::<Vec<_>>();
        server
            .mock_async(move |when, then| {
                when.method("GET")
                    .path(path)
                    .header("Authorization", "Bearer synthetic-test-token")
                    .is_true(move |request| {
                        let actual_query = request.query_params();
                        actual_query.len() == expected_query.len()
                            && expected_query
                                .iter()
                                .all(|expected| actual_query.contains(expected))
                    });
                then.status(200)
                    .header("content-type", "application/json")
                    .body(body);
            })
            .await
    }

    async fn wait_for_hits(mock: &httpmock::Mock<'_>, expected: usize) {
        for _ in 0..100 {
            if mock.calls_async().await == expected {
                return;
            }
            smol::Timer::after(Duration::from_millis(10)).await;
        }
        assert_eq!(mock.calls_async().await, expected, "mock request count");
    }

    async fn wait_for_idle(provider: &GraphProvider) {
        for _ in 0..100 {
            if provider.active_requests() == 0 {
                return;
            }
            smol::Timer::after(Duration::from_millis(10)).await;
        }
        assert_eq!(provider.active_requests(), 0, "provider tasks must stop");
    }

    fn assert_calendar_delta_records(
        first: &[CalendarChange],
        next: &[CalendarChange],
        actual_cursor: &str,
        expected_cursor: &str,
    ) {
        assert_eq!(first.len(), 2);
        assert_eq!(next.len(), 2);
        assert_eq!(
            first[0],
            CalendarChange::Upsert {
                id: synthetic_event_id("event-1"),
                subject: Some("Synthetic meeting".to_owned()),
            }
        );
        assert_eq!(
            next[1],
            CalendarChange::Removed {
                id: synthetic_event_id("event-4"),
                reason: Some("deleted".to_owned()),
            }
        );
        assert_eq!(actual_cursor, expected_cursor);
    }

    fn assert_message_delta_records(
        first: &[MessageChange],
        next: &[MessageChange],
        actual_cursor: &str,
        expected_cursor: &str,
    ) {
        assert_eq!(first.len(), 2);
        let mut subject_patch = MessageMetadataPatch::new();
        subject_patch.set_subject(Some("Synthetic subject".to_owned()));
        let empty_patch = MessageMetadataPatch::new();
        let mut cleared_subject_patch = MessageMetadataPatch::new();
        cleared_subject_patch.set_subject(None);
        assert_eq!(
            first[0],
            MessageChange::Upsert {
                id: synthetic_message_id("message-1"),
                patch: subject_patch,
            }
        );
        assert_eq!(
            first[1],
            MessageChange::Upsert {
                id: synthetic_message_id("message-2"),
                patch: empty_patch,
            }
        );
        assert_eq!(
            next[0],
            MessageChange::Upsert {
                id: synthetic_message_id("message-3"),
                patch: cleared_subject_patch,
            }
        );
        assert_eq!(actual_cursor, expected_cursor);
    }

    #[test]
    fn message_delta_preserves_omitted_fields() {
        let mut metadata = synthetic_message_metadata();
        let original = metadata.clone();

        apply_synthetic_message_delta(&mut metadata, serde_json::json!({ "id": "message-1" }));

        assert_eq!(metadata, original, "omitted fields stay unchanged");
    }

    #[test]
    fn message_delta_updates_only_present_fields() {
        let old_sender = synthetic_mailbox("old-sender@example.test", None);
        let mut metadata = synthetic_message_metadata();

        apply_synthetic_message_delta(
            &mut metadata,
            serde_json::json!({
                "id": "message-1",
                "subject": "Updated subject",
                "from": { "emailAddress": { "address": "new-from@example.test" } },
                "receivedDateTime": "2026-10-31T10:00:00Z",
                "isRead": false
            }),
        );

        assert_eq!(metadata.subject(), Some("Updated subject"));
        assert_eq!(
            metadata.origin().from_mailbox(),
            Some(&synthetic_mailbox("new-from@example.test", None))
        );
        assert_eq!(metadata.origin().sender(), Some(&old_sender));
        assert_eq!(metadata.read_state(), ReadState::Unread);
        assert_eq!(
            metadata.received_at(),
            &parse_received_at("2026-10-31T10:00:00Z").expect("synthetic timestamp is valid")
        );
    }

    #[test]
    fn message_delta_null_clears_nullable_fields() {
        let mut metadata = synthetic_message_metadata();

        apply_synthetic_message_delta(
            &mut metadata,
            serde_json::json!({
                "id": "message-1",
                "subject": null,
                "from": null,
                "sender": null
            }),
        );

        assert_eq!(metadata.subject(), None);
        assert_eq!(metadata.origin().from_mailbox(), None);
        assert_eq!(metadata.origin().sender(), None);
        assert_eq!(metadata.read_state(), ReadState::Read);
    }

    #[test]
    fn message_delta_rejects_null_required_metadata() {
        for value in [
            serde_json::json!({ "id": "message-1", "receivedDateTime": null }),
            serde_json::json!({ "id": "message-1", "isRead": null }),
        ] {
            let wire: WireMessageChange =
                serde_json::from_value(value).expect("null required fields parse at the wire");
            assert_eq!(
                MessageChange::try_from(wire),
                Err(ProviderError::InvalidRecord)
            );
        }
    }

    #[test]
    fn message_delta_rejects_empty_ids_for_upserts_and_removals() {
        for payload in [
            serde_json::json!({ "id": "" }),
            serde_json::json!({ "id": "", "@removed": {} }),
        ] {
            let wire: WireMessageChange =
                serde_json::from_value(payload).expect("empty ID is valid wire text");
            assert_eq!(
                MessageChange::try_from(wire),
                Err(ProviderError::InvalidRecord)
            );
        }
    }

    #[test]
    fn calendar_delta_rejects_empty_ids_for_upserts_and_removals() {
        for payload in [
            serde_json::json!({ "id": "" }),
            serde_json::json!({ "id": "", "@removed": {} }),
        ] {
            let wire: WireCalendarEvent =
                serde_json::from_value(payload).expect("empty ID is valid wire text");
            assert_eq!(
                CalendarChange::try_from(wire),
                Err(ProviderError::InvalidRecord)
            );
        }
    }

    fn assert_synthetic_folder_tree(folders: &[MailFolder]) {
        assert_eq!(
            folders
                .iter()
                .map(|folder| {
                    (
                        folder.reference().folder_id().as_str(),
                        folder.parent().map(|parent| parent.folder_id().as_str()),
                    )
                })
                .collect::<Vec<_>>(),
            [
                ("folder-1", None),
                ("folder-2", None),
                ("nested-1", Some("folder-1")),
                ("grandchild-1", Some("nested-1")),
            ]
        );
        assert!(
            folders
                .iter()
                .all(|folder| { folder.reference().account_id().as_str() == "synthetic-account" })
        );
        assert_eq!(folders[0].display_name(), "Inbox");
        assert_eq!(folders[1].display_name(), "Archive");
        assert_eq!(folders[2].display_name(), "Receipts");
        assert_eq!(folders[3].display_name(), "2026");
    }

    fn assert_synthetic_message_metadata(messages: &[MailMessage]) {
        let folder = synthetic_folder_reference("folder-1");
        let received_at = synthetic_received_at();

        assert_eq!(
            messages,
            [
                MailMessage::new(
                    folder.clone(),
                    MessageId::try_from("message-1".to_owned())
                        .expect("synthetic message ID is non-empty"),
                    MessageMetadata::new(
                        Some("Synthetic subject".to_owned()),
                        MessageOrigin::new(
                            Some(synthetic_mailbox("from@example.test", Some("From Display"),)),
                            Some(synthetic_mailbox(
                                "sender@example.test",
                                Some("Sender Display"),
                            )),
                        ),
                        received_at,
                        ReadState::Read,
                    ),
                ),
                MailMessage::new(
                    folder.clone(),
                    MessageId::try_from("message-2".to_owned())
                        .expect("synthetic message ID is non-empty"),
                    MessageMetadata::new(
                        None,
                        MessageOrigin::new(None, None),
                        received_at,
                        ReadState::Unread,
                    ),
                ),
                MailMessage::new(
                    folder,
                    MessageId::try_from("message-3".to_owned())
                        .expect("synthetic message ID is non-empty"),
                    MessageMetadata::new(
                        Some("Another synthetic subject".to_owned()),
                        MessageOrigin::new(
                            Some(synthetic_mailbox("sender@example.test", None)),
                            None,
                        ),
                        received_at,
                        ReadState::Read,
                    ),
                ),
            ]
        );
    }

    #[test]
    fn graph_deployments_map_to_documented_v1_roots() {
        let deployments = [
            (GraphDeployment::Global, "https://graph.microsoft.com"),
            (
                GraphDeployment::UsGovernmentL4,
                "https://graph.microsoft.us",
            ),
            (
                GraphDeployment::UsGovernmentL5,
                "https://dod-graph.microsoft.us",
            ),
            (
                GraphDeployment::China21Vianet,
                "https://microsoftgraph.chinacloudapi.cn",
            ),
        ];

        for (deployment, service_root) in deployments {
            let authority = GraphAuthority::for_deployment(deployment)
                .expect("documented Microsoft Graph root is valid");
            let request_url = authority
                .initial_path("me/mailFolders")
                .expect("initial Graph path is valid");
            assert_eq!(
                request_url.as_str(),
                format!("{service_root}/v1.0/me/mailFolders")
            );
        }
    }

    #[test]
    fn opaque_links_stay_within_the_selected_graph_deployment() {
        let authority = GraphAuthority::for_deployment(GraphDeployment::Global)
            .expect("global Graph deployment is valid");
        let raw =
            "https://graph.microsoft.com/v1.0/me/messages/delta?$deltatoken=opaque%2Fvalue%3D";
        let accepted = authority
            .opaque_link(raw)
            .expect("same-authority delta cursor is accepted");

        assert_eq!(accepted.as_str(), raw);
        for unsafe_link in [
            "http://graph.microsoft.com/v1.0/me/messages",
            "https://attacker.example/v1.0/me/messages",
            "https://graph.microsoft.us/v1.0/me/messages",
            "https://@graph.microsoft.com/v1.0/me/messages",
            "https://graph.microsoft.com/beta/me/messages",
            "/v1.0/me/messages",
        ] {
            assert!(
                matches!(
                    authority.opaque_link(unsafe_link),
                    Err(ProviderError::UnsafeLink)
                ),
                "unsafe Graph link accepted: {unsafe_link}"
            );
        }
    }

    #[test]
    fn loopback_mock_authority_rejects_non_loopback_origins() {
        for untrusted_origin in [
            "http://graph.microsoft.com",
            "https://127.0.0.1:1234",
            "http://@127.0.0.1:1234",
        ] {
            assert!(matches!(
                GraphAuthority::for_loopback_mock(untrusted_origin),
                Err(ProviderError::InvalidUrl)
            ));
        }
    }

    #[test]
    fn authorization_header_is_sensitive_and_contains_only_the_synthetic_token() {
        let header = authorization_header(&BearerToken::synthetic())
            .expect("synthetic bearer token is a valid header value");

        assert!(header.is_sensitive());
        assert_eq!(
            header
                .to_str()
                .expect("authorization header is valid UTF-8"),
            "Bearer synthetic-test-token"
        );
    }

    #[test]
    fn response_body_chunks_cannot_exceed_the_limit() {
        let mut body = vec![0; MAX_RESPONSE_BODY_BYTES];

        assert_eq!(
            append_response_chunk(&mut body, b"x"),
            Err(ProviderError::ResponseTooLarge)
        );
        assert_eq!(body.len(), MAX_RESPONSE_BODY_BYTES);
    }

    #[test]
    fn pagination_budget_limits_pages_and_cumulative_bytes() {
        let mut page_budget = PaginationBudget::default();
        for _ in 0..MAX_PAGINATION_PAGES {
            page_budget
                .begin_page()
                .expect("page budget accepts its configured limit");
        }
        assert_eq!(
            page_budget.begin_page(),
            Err(ProviderError::PaginationLimitExceeded)
        );

        let mut byte_budget = PaginationBudget::default();
        byte_budget
            .add_response_bytes(MAX_PAGINATION_RESPONSE_BYTES)
            .expect("byte budget accepts its configured limit");
        assert_eq!(
            byte_budget.add_response_bytes(1),
            Err(ProviderError::PaginationLimitExceeded)
        );
        assert_eq!(byte_budget.response_bytes, MAX_PAGINATION_RESPONSE_BYTES);
    }

    #[test]
    fn oversized_chunked_graph_response_is_rejected_without_content_length() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let oversized_body = vec![b'x'; MAX_RESPONSE_BODY_BYTES + 1];
            let oversized_page = server
                .mock_async(move |when, then| {
                    when.method("GET").path("/v1.0/me/mailFolders");
                    then.status(200)
                        .header("transfer-encoding", "chunked")
                        .body(oversized_body);
                })
                .await;

            let response_length = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test HTTP runtime builds")
                .block_on(async {
                    build_http_client(REQUEST_TIMEOUT)
                        .expect("HTTP client builds")
                        .get(format!("{}/v1.0/me/mailFolders", server.base_url()))
                        .send()
                        .await
                        .expect("chunked mock response arrives")
                        .content_length()
                });
            assert!(response_length.is_none());

            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");

            assert_eq!(
                provider
                    .folders(&synthetic_account_id(), &BearerToken::synthetic())
                    .await,
                Err(ProviderError::ResponseTooLarge)
            );
            oversized_page.assert_calls_async(2).await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn folder_pages_follow_links_and_traverse_the_visible_tree() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let root_next_link = format!(
                "{}/v1.0/me/mailFolders?$select={FOLDER_SELECT}&page=2&$skiptoken=opaque%2Fpage%3D",
                server.base_url()
            );
            let first_root_body = serde_json::json!({
                "value": [{ "id": "folder-1", "displayName": "Inbox" }],
                "@odata.nextLink": root_next_link,
            })
            .to_string();
            let second_root_body = serde_json::json!({
                "value": [{ "id": "folder-2", "displayName": "Archive" }],
            })
            .to_string();
            let nested_body = serde_json::json!({
                "value": [{ "id": "nested-1", "displayName": "Receipts" }],
            })
            .to_string();
            let grandchild_body = serde_json::json!({
                "value": [{ "id": "grandchild-1", "displayName": "2026" }],
            })
            .to_string();
            let first_root_page = mock_folder_page(
                &server,
                "/v1.0/me/mailFolders",
                vec![("$select", FOLDER_SELECT)],
                first_root_body,
            )
            .await;
            let second_root_page = mock_folder_page(
                &server,
                "/v1.0/me/mailFolders",
                vec![
                    ("$select", FOLDER_SELECT),
                    ("page", "2"),
                    ("$skiptoken", "opaque/page="),
                ],
                second_root_body,
            )
            .await;
            let folder_one_children = mock_folder_page(
                &server,
                "/v1.0/me/mailFolders/folder-1/childFolders",
                vec![("$select", FOLDER_SELECT)],
                nested_body,
            )
            .await;
            let folder_two_children = mock_folder_page(
                &server,
                "/v1.0/me/mailFolders/folder-2/childFolders",
                vec![("$select", FOLDER_SELECT)],
                r#"{"value":[]}"#.to_owned(),
            )
            .await;
            let nested_children = mock_folder_page(
                &server,
                "/v1.0/me/mailFolders/nested-1/childFolders",
                vec![("$select", FOLDER_SELECT)],
                grandchild_body,
            )
            .await;
            let grandchild_children = mock_folder_page(
                &server,
                "/v1.0/me/mailFolders/grandchild-1/childFolders",
                vec![("$select", FOLDER_SELECT)],
                r#"{"value":[]}"#.to_owned(),
            )
            .await;

            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");
            let folders = provider
                .folders(&synthetic_account_id(), &BearerToken::synthetic())
                .await
                .expect("synthetic folder pages parse");

            assert_synthetic_folder_tree(&folders);
            first_root_page.assert_async().await;
            second_root_page.assert_async().await;
            folder_one_children.assert_async().await;
            folder_two_children.assert_async().await;
            nested_children.assert_async().await;
            grandchild_children.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn folder_traversal_rejects_duplicate_ids_in_a_tree() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let root_body = serde_json::json!({
                "value": [{ "id": "folder-1", "displayName": "Inbox" }],
            })
            .to_string();
            let duplicate_body = serde_json::json!({
                "value": [{ "id": "folder-1", "displayName": "Duplicate" }],
            })
            .to_string();
            let root = mock_folder_page(
                &server,
                "/v1.0/me/mailFolders",
                vec![("$select", FOLDER_SELECT)],
                root_body,
            )
            .await;
            let duplicate_child = mock_folder_page(
                &server,
                "/v1.0/me/mailFolders/folder-1/childFolders",
                vec![("$select", FOLDER_SELECT)],
                duplicate_body,
            )
            .await;
            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");

            assert_eq!(
                provider
                    .folders(&synthetic_account_id(), &BearerToken::synthetic())
                    .await,
                Err(ProviderError::InvalidRecord)
            );
            root.assert_async().await;
            duplicate_child.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn initial_message_delta_path_scopes_folder_and_metadata() {
        let authority = GraphAuthority::for_deployment(GraphDeployment::Global)
            .expect("global Graph deployment is valid");
        let path = authority
            .initial_message_delta_path("folder/one", "2026-08-02T00:00:00Z")
            .expect("valid folder ID and sync cutoff produce a Graph URL");
        let url = Url::parse(path.as_str()).expect("constructed Graph URL is valid");
        let query = url.query_pairs().collect::<Vec<_>>();

        assert_eq!(
            url.path(),
            "/v1.0/me/mailFolders/folder%2Fone/messages/delta"
        );
        assert!(query.contains(&("$select".into(), MESSAGE_SELECT.into(),)));
        assert!(query.contains(&(
            "$filter".into(),
            "receivedDateTime ge 2026-08-02T00:00:00Z".into(),
        )));
    }

    #[test]
    fn message_pages_preserve_immutable_ids_and_parse_synthetic_records() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let next_link = format!(
                "{}/v1.0/me/mailFolders/folder-1/messages?$select={MESSAGE_SELECT}&page=2&$skiptoken=opaque%2Fmessages",
                server.base_url()
            );
            let first_body = serde_json::json!({
                "value": [
                    {
                        "id": "message-1",
                        "subject": "Synthetic subject",
                        "from": { "emailAddress": { "name": "From Display", "address": "from@example.test" } },
                        "sender": { "emailAddress": { "name": "Sender Display", "address": "sender@example.test" } },
                        "receivedDateTime": "2026-10-30T14:00:00+02:00",
                        "isRead": true
                    },
                    {
                        "id": "message-2",
                        "subject": null,
                        "from": null,
                        "sender": null,
                        "receivedDateTime": "2026-10-30T12:00:00Z",
                        "isRead": false
                    },
                ],
                "@odata.nextLink": next_link,
            })
            .to_string();
            let second_body = serde_json::json!({
                "value": [{
                    "id": "message-3",
                    "subject": "Another synthetic subject",
                    "from": { "emailAddress": { "address": "sender@example.test" } },
                    "receivedDateTime": "2026-10-30T12:00:00Z",
                    "isRead": true
                }],
            })
            .to_string();

            let first_page = mock_message_page(
                &server,
                "/v1.0/me/mailFolders/folder-1/messages",
                vec![("$select", MESSAGE_SELECT)],
                first_body,
            )
            .await;
            let second_page = mock_message_page(
                &server,
                "/v1.0/me/mailFolders/folder-1/messages",
                vec![
                    ("$select", MESSAGE_SELECT),
                    ("page", "2"),
                    ("$skiptoken", "opaque/messages"),
                ],
                second_body,
            )
            .await;

            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");
            let messages = provider
                .messages(
                    &synthetic_folder_reference("folder-1"),
                    &BearerToken::synthetic(),
                )
                .await
                .expect("synthetic message pages parse");

            assert_synthetic_message_metadata(&messages);
            first_page.assert_async().await;
            second_page.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn received_date_time_parser_normalizes_offsets_and_rejects_invalid_values() {
        let expected = synthetic_received_at();

        assert_eq!(parse_received_at("2026-10-30T14:00:00+02:00"), Ok(expected));
        assert_eq!(
            parse_received_at("not-a-date"),
            Err(ProviderError::InvalidRecord)
        );
    }

    #[test]
    fn empty_graph_mailbox_addresses_fail_at_the_boundary() {
        let recipient = WireRecipient {
            email_address: WireEmailAddress {
                address: " \t ".to_owned(),
                name: Some("Synthetic sender".to_owned()),
            },
        };

        assert_eq!(
            Mailbox::try_from(recipient),
            Err(ProviderError::InvalidRecord)
        );
    }

    #[test]
    fn repeated_folder_page_links_are_rejected_before_a_second_request() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let repeated_link = format!(
                "{}/v1.0/me/mailFolders?$select={FOLDER_SELECT}",
                server.base_url()
            );
            let repeated_page_body = serde_json::json!({
                "value": [],
                "@odata.nextLink": repeated_link,
            })
            .to_string();
            let repeated_page = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/mailFolders")
                        .query_param("$select", FOLDER_SELECT);
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(repeated_page_body);
                })
                .await;
            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");

            assert_eq!(
                provider
                    .folders(&synthetic_account_id(), &BearerToken::synthetic())
                    .await,
                Err(ProviderError::UnsafeLink)
            );
            wait_for_hits(&repeated_page, 1).await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn delta_rounds_keep_immutable_ids_and_reuse_the_opaque_delta_link() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let base_path = "/v1.0/me/mailFolders/folder-1/messages/delta";
            let query = format!(
                "$select={MESSAGE_SELECT}&$filter=receivedDateTime%20ge%202026-08-02T00%3A00%3A00Z"
            );
            let base_delta_url = format!("{}{base_path}?{query}", server.base_url());
            let next_link = format!("{base_delta_url}&page=2&$skiptoken=opaque%2Fnext");
            let first_cursor = format!("{base_delta_url}&$deltatoken=opaque%2Fone");
            let second_cursor = format!("{base_delta_url}&$deltatoken=opaque%2Ftwo");

            let first_body = serde_json::json!({
                "value": [{ "id": "message-1", "subject": "Synthetic subject" }],
                "@odata.nextLink": next_link,
            })
            .to_string();
            let second_body = serde_json::json!({
                "value": [{ "id": "message-2" }],
                "@odata.deltaLink": first_cursor.clone(),
            })
            .to_string();
            let next_round_body = serde_json::json!({
                "value": [{ "id": "message-3", "subject": null }],
                "@odata.deltaLink": second_cursor.clone(),
            })
            .to_string();

            let first_page = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path(base_path)
                        .query_param("$select", MESSAGE_SELECT)
                        .query_param("$filter", "receivedDateTime ge 2026-08-02T00:00:00Z")
                        .is_true(|request| request.query_params().len() == 2)
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(first_body);
                })
                .await;
            let second_page = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path(base_path)
                        .query_param("$select", MESSAGE_SELECT)
                        .query_param("$filter", "receivedDateTime ge 2026-08-02T00:00:00Z")
                        .query_param("page", "2")
                        .query_param("$skiptoken", "opaque/next")
                        .is_true(|request| request.query_params().len() == 4)
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(second_body);
                })
                .await;
            let next_round = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path(base_path)
                        .query_param("$select", MESSAGE_SELECT)
                        .query_param("$filter", "receivedDateTime ge 2026-08-02T00:00:00Z")
                        .query_param("$deltatoken", "opaque/one")
                        .is_true(|request| request.query_params().len() == 3)
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(next_round_body);
                })
                .await;

            let authority = loopback_authority(&server);
            let provider = GraphProvider::start(authority.clone())
                .await
                .expect("provider worker starts");
            let first_start = authority
                .initial_message_delta_path("folder-1", "2026-08-02T00:00:00Z")
                .expect("synthetic folder delta path is valid");
            let first = provider
                .message_delta(first_start, &BearerToken::synthetic())
                .await
                .expect("first delta round parses");
            let second = provider
                .message_delta(first.delta_link, &BearerToken::synthetic())
                .await
                .expect("next delta round parses");

            assert_message_delta_records(
                &first.records,
                &second.records,
                second.delta_link.as_str(),
                &second_cursor,
            );
            first_page.assert_async().await;
            second_page.assert_async().await;
            next_round.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn message_delta_maps_removed_records_with_optional_reason() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let endpoint = "/v1.0/me/mailFolders/folder-1/messages/delta";
            let delta_link = format!(
                "{}{endpoint}?$select={MESSAGE_SELECT}&$filter=receivedDateTime%20ge%202026-08-02T00%3A00%3A00Z&$deltatoken=opaque%2Fdeleted",
                server.base_url()
            );
            let body = serde_json::json!({
                "value": [
                    { "id": "message-removed", "@removed": { "reason": "deleted" } },
                    { "id": "message-removed-no-reason", "@removed": {} },
                ],
                "@odata.deltaLink": delta_link.clone(),
            })
            .to_string();
            let response = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path(endpoint)
                        .query_param("$select", MESSAGE_SELECT)
                        .query_param("$filter", "receivedDateTime ge 2026-08-02T00:00:00Z")
                        .is_true(|request| request.query_params().len() == 2)
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(body);
                })
                .await;
            let authority = loopback_authority(&server);
            let provider = GraphProvider::start(authority.clone())
                .await
                .expect("provider worker starts");
            let start = authority
                .initial_message_delta_path("folder-1", "2026-08-02T00:00:00Z")
                .expect("synthetic folder delta path is valid");
            let changes = provider
                .message_delta(start, &BearerToken::synthetic())
                .await
                .expect("removed message records parse");

            assert_eq!(changes.records.len(), 2);
            assert_eq!(
                changes.records[0],
                MessageChange::Removed {
                    id: synthetic_message_id("message-removed"),
                    reason: Some("deleted".to_owned()),
                }
            );
            assert_eq!(
                changes.records[1],
                MessageChange::Removed {
                    id: synthetic_message_id("message-removed-no-reason"),
                    reason: None,
                }
            );
            assert_eq!(changes.delta_link.as_str(), delta_link);
            response.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn calendar_view_delta_preserves_event_ids_and_deletions() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let query =
                "startDateTime=2026-10-31T00%3A00%3A00Z&endDateTime=2026-11-01T00%3A00%3A00Z";
            let next_link = format!(
                "{}/v1.0/me/calendarView/delta?{query}&page=2&$skiptoken=opaque%2Fcalendar",
                server.base_url()
            );
            let first_cursor = format!(
                "{}/v1.0/me/calendarView/delta?{query}&$deltatoken=opaque%2Ffirst",
                server.base_url()
            );
            let next_cursor = format!(
                "{}/v1.0/me/calendarView/delta?{query}&$deltatoken=opaque%2Fnext",
                server.base_url()
            );
            let first_body = serde_json::json!({
                "value": [{ "id": "event-1", "subject": "Synthetic meeting" }],
                "@odata.nextLink": next_link,
            })
            .to_string();
            let second_body = serde_json::json!({
                "value": [{ "id": "event-2", "subject": "Another meeting" }],
                "@odata.deltaLink": first_cursor.clone(),
            })
            .to_string();
            let next_round_body = serde_json::json!({
                "value": [
                    { "id": "event-3", "subject": "Updated meeting" },
                    { "id": "event-4", "@removed": { "reason": "deleted" } },
                ],
                "@odata.deltaLink": next_cursor.clone(),
            })
            .to_string();

            let first_page = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/calendarView/delta")
                        .query_param("startDateTime", "2026-10-31T00:00:00Z")
                        .query_param("endDateTime", "2026-11-01T00:00:00Z")
                        .header("Prefer", "IdType=\"ImmutableId\"")
                        .is_true(|request| request.query_params().len() == 2);
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(first_body);
                })
                .await;
            let second_page = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/calendarView/delta")
                        .query_param("page", "2")
                        .query_param("$skiptoken", "opaque/calendar")
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(second_body);
                })
                .await;
            let next_round = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/calendarView/delta")
                        .query_param("$deltatoken", "opaque/first")
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(next_round_body);
                })
                .await;

            let authority = loopback_authority(&server);
            let provider = GraphProvider::start(authority.clone())
                .await
                .expect("provider worker starts");
            let start = authority
                .initial_path(&format!("me/calendarView/delta?{query}"))
                .expect("synthetic calendar delta path is valid");
            let first = provider
                .calendar_view_delta(start, &BearerToken::synthetic())
                .await
                .expect("calendar delta pages parse");
            let second = provider
                .calendar_view_delta(first.delta_link, &BearerToken::synthetic())
                .await
                .expect("next calendar delta round parses");

            assert_calendar_delta_records(
                &first.records,
                &second.records,
                second.delta_link.as_str(),
                &next_cursor,
            );
            first_page.assert_async().await;
            second_page.assert_async().await;
            next_round.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn malformed_json_and_invalid_ids_fail_at_the_graph_boundary() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let malformed = server
                .mock_async(move |when, then| {
                    when.method("GET").path("/v1.0/me/mailFolders");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body("{");
                })
                .await;
            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");
            assert_eq!(
                provider
                    .folders(&synthetic_account_id(), &BearerToken::synthetic())
                    .await,
                Err(ProviderError::MalformedResponse)
            );
            malformed.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");

            let server = MockServer::start_async().await;
            let empty_id = server
                .mock_async(move |when, then| {
                    when.method("GET").path("/v1.0/me/mailFolders");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(r#"{"value":[{"id":"","displayName":"Inbox"}]}"#);
                })
                .await;
            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");
            assert_eq!(
                provider
                    .folders(&synthetic_account_id(), &BearerToken::synthetic())
                    .await,
                Err(ProviderError::InvalidRecord)
            );
            empty_id.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn unsafe_next_links_are_rejected_before_another_authorized_request() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let body = serde_json::json!({
                "value": [],
                "@odata.nextLink": "https://attacker.example/collect",
            })
            .to_string();
            let first_page = server
                .mock_async(move |when, then| {
                    when.method("GET").path("/v1.0/me/mailFolders");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(body);
                })
                .await;
            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");

            assert_eq!(
                provider
                    .folders(&synthetic_account_id(), &BearerToken::synthetic())
                    .await,
                Err(ProviderError::UnsafeLink)
            );
            first_page.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn reqwest_does_not_follow_an_authorized_redirect() {
        smol::block_on(async {
            let target = MockServer::start_async().await;
            let target_mock = target
                .mock_async(|when, then| {
                    when.any_request();
                    then.status(200).body("synthetic redirect target");
                })
                .await;
            let source = MockServer::start_async().await;
            let target_url = target.url("/collect");
            let redirect = source
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/messages")
                        .header("Authorization", "Bearer synthetic-test-token")
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(302).header("Location", target_url);
                })
                .await;
            let authority = loopback_authority(&source);
            let provider = GraphProvider::start(authority.clone())
                .await
                .expect("provider worker starts");
            let message_url = authority
                .initial_path("me/messages")
                .expect("synthetic message path is valid");

            assert_eq!(
                provider
                    .get(
                        GraphOperation::Messages,
                        message_url,
                        &BearerToken::synthetic(),
                    )
                    .await,
                Err(ProviderError::Rejected)
            );
            redirect.assert_async().await;
            target_mock.assert_calls_async(0).await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn reqwest_enforces_the_total_request_deadline() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let delayed_response = server
                .mock_async(|when, then| {
                    when.method("GET").path("/v1.0/slow");
                    then.status(200).delay(Duration::from_secs(1));
                })
                .await;
            let test_deadline = Duration::from_millis(100);
            let request_url = format!("{}/v1.0/slow", server.base_url());
            let client = build_http_client(test_deadline).expect("HTTP client builds");
            let timeout_error = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test HTTP runtime builds")
                .block_on(async { client.get(request_url).send().await })
                .expect_err("delayed synthetic response exceeds the deadline");

            assert!(
                timeout_error.is_timeout(),
                "expected timeout error, got {timeout_error}"
            );
            delayed_response.assert_async().await;
        });
    }

    #[test]
    fn retry_after_seconds_http_dates_and_fallback_backoff_are_parsed() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let seconds = HeaderValue::from_static("7");
        let retry_at = now + Duration::from_secs(5);
        let date = httpdate::fmt_http_date(retry_at);
        let date = HeaderValue::from_bytes(date.as_bytes())
            .expect("formatted HTTP date is a valid header");

        assert_eq!(
            retry_after_delay(Some(&seconds), now, 0),
            RetryAfter::Delay(Duration::from_secs(7))
        );
        assert_eq!(
            retry_after_delay(Some(&date), now, 0),
            RetryAfter::Delay(Duration::from_secs(5))
        );
        assert_eq!(
            retry_after_delay(None, now, 0),
            RetryAfter::Delay(Duration::from_secs(1))
        );
        assert_eq!(
            retry_after_delay(None, now, 5),
            RetryAfter::Delay(Duration::from_secs(32))
        );
        assert_eq!(
            retry_after_delay(None, now, 99),
            RetryAfter::Delay(Duration::from_mins(1))
        );
    }

    #[test]
    fn automatic_retry_returns_long_delays_and_exhaustion_to_the_caller() {
        let maximum_delay = MAX_AUTOMATIC_RETRY_DELAY;
        assert_eq!(
            automatic_retry_delay(RetryAfter::Delay(maximum_delay), 0),
            Ok(maximum_delay)
        );

        let long_delay = maximum_delay + Duration::from_secs(1);
        assert_eq!(
            automatic_retry_delay(RetryAfter::Delay(long_delay), 0),
            Err(ProviderError::Throttled {
                retry_after: RetryAfter::Delay(long_delay),
            })
        );

        let remaining_delay = Duration::from_secs(4);
        assert_eq!(
            automatic_retry_delay(RetryAfter::Delay(remaining_delay), MAX_THROTTLE_RETRIES),
            Err(ProviderError::Throttled {
                retry_after: RetryAfter::Delay(remaining_delay),
            })
        );
    }

    #[test]
    fn retry_after_seconds_larger_than_u64_are_unrepresentable() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let too_large = HeaderValue::from_static("18446744073709551616");

        assert_eq!(
            retry_after_delay(Some(&too_large), now, 0),
            RetryAfter::Unrepresentable
        );
    }

    #[test]
    fn graph_throttling_retries_only_the_429_response() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let calls = Arc::new(AtomicUsize::new(0));
            let responder_calls = Arc::clone(&calls);
            let throttle = server
                .mock_async(move |when, then| {
                    when.method("GET").path("/v1.0/me/mailFolders");
                    then.respond_with(move |_request: &HttpMockRequest| {
                        if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            HttpMockResponse::builder()
                                .status(429)
                                .header("Retry-After", "0")
                                .body("synthetic throttle response")
                                .build()
                        } else {
                            HttpMockResponse::builder()
                                .status(200)
                                .header("content-type", "application/json")
                                .body(r#"{"value":[]}"#)
                                .build()
                        }
                    });
                })
                .await;
            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");

            assert_eq!(
                provider
                    .folders(&synthetic_account_id(), &BearerToken::synthetic())
                    .await
                    .expect("retry after a synthetic 429 succeeds"),
                Vec::<MailFolder>::new()
            );
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            throttle.assert_calls_async(2).await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn unauthorized_responses_are_not_retried() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let unauthorized = server
                .mock_async(|when, then| {
                    when.method("GET").path("/v1.0/me/mailFolders");
                    then.status(401);
                })
                .await;
            let forbidden = server
                .mock_async(|when, then| {
                    when.method("GET").path("/v1.0/me/messages");
                    then.status(403);
                })
                .await;
            let authority = loopback_authority(&server);
            let provider = GraphProvider::start(authority.clone())
                .await
                .expect("provider worker starts");

            assert_eq!(
                provider
                    .folders(&synthetic_account_id(), &BearerToken::synthetic())
                    .await,
                Err(ProviderError::Rejected)
            );
            let message_url = authority
                .initial_path("me/messages")
                .expect("synthetic message path is valid");
            assert_eq!(
                provider
                    .get(
                        GraphOperation::Messages,
                        message_url,
                        &BearerToken::synthetic(),
                    )
                    .await,
                Err(ProviderError::Rejected)
            );

            unauthorized.assert_async().await;
            forbidden.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn graph_error_body_is_not_returned_or_retried() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let graph_error = server
                .mock_async(|when, then| {
                    when.method("GET").path("/v1.0/me/messages/error");
                    then.status(400)
                        .header("content-type", "application/json")
                        .body(r#"{"error":{"code":"InvalidRequest","message":"synthetic error"}}"#);
                })
                .await;
            let authority = loopback_authority(&server);
            let provider = GraphProvider::start(authority.clone())
                .await
                .expect("provider worker starts");
            let error_url = authority
                .initial_path("me/messages/error")
                .expect("synthetic error path is valid");

            assert_eq!(
                provider
                    .get(
                        GraphOperation::Messages,
                        error_url,
                        &BearerToken::synthetic(),
                    )
                    .await,
                Err(ProviderError::Rejected)
            );
            assert_eq!(graph_error.calls_async().await, 1);
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn smol_stays_responsive_during_io_and_shutdown_releases_the_worker() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let slow = server
                .mock_async(|when, then| {
                    when.method("GET").path("/v1.0/slow");
                    then.status(200).delay(Duration::from_secs(20));
                })
                .await;
            let shutdown_slow = server
                .mock_async(|when, then| {
                    when.method("GET").path("/v1.0/shutdown-slow");
                    then.status(200).delay(Duration::from_secs(20));
                })
                .await;
            let authority = loopback_authority(&server);
            let provider = GraphProvider::start(authority.clone())
                .await
                .expect("provider worker starts");
            let request = provider
                .begin_get(
                    GraphOperation::Folders,
                    authority
                        .initial_path("slow")
                        .expect("synthetic slow path is valid"),
                    BearerToken::synthetic(),
                )
                .await
                .expect("request is queued");
            wait_for_hits(&slow, 1).await;

            let smol_progress = Arc::new(AtomicUsize::new(0));
            let mark_progress = Arc::clone(&smol_progress);
            smol::spawn(async move {
                smol::Timer::after(Duration::from_millis(10)).await;
                mark_progress.store(1, Ordering::SeqCst);
            })
            .detach();
            while smol_progress.load(Ordering::SeqCst) == 0 {
                smol::Timer::after(Duration::from_millis(1)).await;
            }
            assert_eq!(smol_progress.load(Ordering::SeqCst), 1);
            drop(request);
            wait_for_idle(&provider).await;

            let request = provider
                .begin_get(
                    GraphOperation::Folders,
                    authority
                        .initial_path("shutdown-slow")
                        .expect("synthetic shutdown path is valid"),
                    BearerToken::synthetic(),
                )
                .await
                .expect("second request is queued");
            wait_for_hits(&shutdown_slow, 1).await;
            let requests = Arc::clone(&provider.requests);
            provider
                .shutdown()
                .await
                .expect("shutdown waits for runtime drop");
            drop(request);

            assert_eq!(requests.active(), 0);
        });
    }

    #[cfg(target_os = "linux")]
    #[gpui::test]
    async fn gpui_task_remains_responsive_while_waiting_for_graph_response(
        cx: &mut gpui::TestAppContext,
    ) {
        enum RequestSignal {
            Started,
            Finished,
        }

        let server = smol::block_on(MockServer::start_async());
        let (signals, signal_rx) = bounded(2);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let response_signals = signals.clone();
        let release_rx = std::sync::Mutex::new(release_rx);
        let response = smol::block_on(server.mock_async(move |when, then| {
            when.method("GET").path("/v1.0/me/mailFolders");
            then.respond_with(move |_request: &HttpMockRequest| {
                let _ = response_signals.try_send(RequestSignal::Started);
                let _ = release_rx
                    .lock()
                    .expect("synthetic response gate is not poisoned")
                    .recv_timeout(Duration::from_secs(2));
                HttpMockResponse::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"value":[]}"#)
                    .build()
            });
        }));
        let provider = smol::block_on(GraphProvider::start(loopback_authority(&server)))
            .expect("provider worker starts");

        cx.executor().allow_parking();
        let request_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mark_request_finished = Arc::clone(&request_finished);
        let task_signals = signals.clone();
        let request_task = cx.spawn(move |_| async move {
            let folders = provider
                .folders(&synthetic_account_id(), &BearerToken::synthetic())
                .await;
            mark_request_finished.store(true, Ordering::SeqCst);
            let _ = task_signals.try_send(RequestSignal::Finished);
            let shutdown = provider.shutdown().await;
            folders.and_then(|folders| shutdown.map(|()| folders))
        });

        let release_response = release_tx.clone();
        let progress_task = cx.spawn(move |_| async move {
            let progressed_during_request = match signal_rx.recv().await {
                Ok(RequestSignal::Started) => !request_finished.load(Ordering::SeqCst),
                Ok(RequestSignal::Finished) | Err(_) => false,
            };
            let _ = release_response.send(());
            progressed_during_request
        });

        assert!(
            progress_task.await,
            "another GPUI task progresses before the Graph response arrives"
        );
        assert_eq!(
            request_task
                .await
                .expect("Graph request and worker shutdown succeed"),
            Vec::<MailFolder>::new()
        );
        response.assert();
    }
}
