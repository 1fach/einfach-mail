use async_channel::{Receiver, Sender, bounded};
use reqwest::{
    Client, Response, StatusCode, Url,
    header::{AUTHORIZATION, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use serde::Deserialize;
use std::{
    collections::HashSet,
    future::Future,
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, SystemTime},
};
use tokio::{runtime::Builder, task::JoinSet};

const MAX_THROTTLE_RETRIES: u32 = 2;
const MAX_FALLBACK_DELAY_SECONDS: u64 = 60;
const REQUEST_CHANNEL_CAPACITY: usize = 16;

#[derive(Debug, Eq, PartialEq)]
enum ProviderError {
    Startup,
    WorkerClosed,
    Transport,
    InvalidToken,
    Rejected,
    Throttled,
    MalformedResponse,
    InvalidUrl,
    UnsafeLink,
    InvalidRecord,
}

#[derive(Clone, Eq, PartialEq)]
struct GraphOrigin {
    scheme: String,
    host: String,
    port: u16,
}

#[derive(Clone)]
struct GraphAuthority {
    base_url: Url,
    origin: GraphOrigin,
}

impl GraphAuthority {
    fn new(base_url: &str) -> Result<Self, ProviderError> {
        let mut base_url = Url::parse(base_url).map_err(|_| ProviderError::InvalidUrl)?;
        if base_url.scheme() != "https" {
            return Err(ProviderError::InvalidUrl);
        }
        let configured_path = base_url.path().trim_end_matches('/');
        if !configured_path.is_empty() && configured_path != "/v1.0" {
            return Err(ProviderError::InvalidUrl);
        }
        base_url.set_path("/v1.0/");
        Self::from_base_url(base_url)
    }

    fn for_loopback_mock(base_url: &str) -> Result<Self, ProviderError> {
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

    fn opaque_link(&self, link: &str) -> Result<GraphUrl, ProviderError> {
        let parsed = Url::parse(link).map_err(|_| ProviderError::UnsafeLink)?;
        self.validate_url(&parsed)?;
        Ok(GraphUrl(link.to_owned()))
    }

    fn validate_url(&self, url: &Url) -> Result<(), ProviderError> {
        if !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
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
            .name("einfach-mail-graph-proof".to_owned())
            .spawn(move || {
                let Ok(runtime) = Builder::new_current_thread().enable_all().build() else {
                    let _ = ready_tx.send_blocking(Err(ProviderError::Startup));
                    let _ = stopped_tx.send_blocking(());
                    return;
                };

                let Ok(client) = build_http_client() else {
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

    async fn folders(&self, token: &BearerToken) -> Result<Vec<Folder>, ProviderError> {
        let mut next = self.authority.initial_path("me/mailFolders")?;
        let mut seen = HashSet::new();
        let mut folders = Vec::new();

        loop {
            if !seen.insert(next.as_str().to_owned()) {
                return Err(ProviderError::UnsafeLink);
            }

            let page = self
                .get_page::<WireFolder>(GraphOperation::Folders, next, token)
                .await?;
            folders.extend(
                page.value
                    .into_iter()
                    .map(Folder::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            );

            match page.next_link {
                Some(link) => next = self.authority.opaque_link(&link)?,
                None => return Ok(folders),
            }
        }
    }

    async fn message_delta(
        &self,
        start: GraphUrl,
        token: &BearerToken,
    ) -> Result<DeltaRound<MessageChange>, ProviderError> {
        self.delta_pages::<WireMessage>(GraphOperation::MessageDelta, start, token)
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
    ) -> Result<GraphPage<T>, ProviderError> {
        let body = self.get(operation, url, token).await?;
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

        loop {
            if !seen.insert(next.as_str().to_owned()) {
                return Err(ProviderError::UnsafeLink);
            }
            let page = self.get_page(operation, next, token).await?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireFolder {
    id: String,
    display_name: String,
}

#[derive(Debug, Eq, PartialEq)]
struct Folder {
    id: String,
    display_name: String,
}

impl TryFrom<WireFolder> for Folder {
    type Error = ProviderError;

    fn try_from(folder: WireFolder) -> Result<Self, Self::Error> {
        if folder.id.is_empty() {
            return Err(ProviderError::InvalidRecord);
        }
        Ok(Self {
            id: folder.id,
            display_name: folder.display_name,
        })
    }
}

#[derive(Deserialize)]
struct WireMessage {
    id: String,
    subject: Option<String>,
    #[serde(rename = "@removed")]
    removed: Option<WireRemoved>,
}

#[derive(Debug, Eq, PartialEq)]
enum MessageChange {
    Upsert { id: String, subject: Option<String> },
    Removed { id: String, reason: Option<String> },
}

impl TryFrom<WireMessage> for MessageChange {
    type Error = ProviderError;

    fn try_from(message: WireMessage) -> Result<Self, Self::Error> {
        if message.id.is_empty() {
            return Err(ProviderError::InvalidRecord);
        }
        match message.removed {
            Some(removed) => Ok(Self::Removed {
                id: message.id,
                reason: removed.reason,
            }),
            None => Ok(Self::Upsert {
                id: message.id,
                subject: message.subject,
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
    Upsert { id: String, subject: Option<String> },
    Removed { id: String, reason: Option<String> },
}

impl TryFrom<WireCalendarEvent> for CalendarChange {
    type Error = ProviderError;

    fn try_from(event: WireCalendarEvent) -> Result<Self, Self::Error> {
        if event.id.is_empty() {
            return Err(ProviderError::InvalidRecord);
        }
        match event.removed {
            Some(removed) => Ok(Self::Removed {
                id: event.id,
                reason: removed.reason,
            }),
            None => Ok(Self::Upsert {
                id: event.id,
                subject: event.subject,
            }),
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

fn build_http_client() -> Result<Client, ProviderError> {
    Client::builder()
        // The proof uses a local mock server; production must retain system proxy support.
        .no_proxy()
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
    for retry_index in 0..=MAX_THROTTLE_RETRIES {
        let response = match send_once(client, &command).await? {
            Ok(response) => response,
            Err(error) => return Some(Err(error)),
        };

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            if retry_index == MAX_THROTTLE_RETRIES {
                return Some(Err(ProviderError::Throttled));
            }

            let delay = retry_after_delay(
                response.headers().get(RETRY_AFTER),
                SystemTime::now(),
                retry_index,
            );
            while_interested(&command.reply, tokio::time::sleep(delay)).await?;
            continue;
        }

        if !response.status().is_success() {
            return Some(Err(ProviderError::Rejected));
        }

        let Ok(body) = while_interested(&command.reply, response.bytes()).await? else {
            return Some(Err(ProviderError::Transport));
        };
        return Some(Ok(body.to_vec()));
    }

    Some(Err(ProviderError::Throttled))
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

fn retry_after_delay(value: Option<&HeaderValue>, now: SystemTime, attempt: u32) -> Duration {
    let Some(value) = value.and_then(|value| value.to_str().ok()) else {
        return fallback_delay(attempt);
    };

    if let Ok(seconds) = value.parse::<u64>() {
        return Duration::from_secs(seconds);
    }

    match httpdate::parse_http_date(value) {
        Ok(retry_at) => retry_at.duration_since(now).unwrap_or(Duration::ZERO),
        Err(_) => fallback_delay(attempt),
    }
}

fn fallback_delay(attempt: u32) -> Duration {
    let seconds = 2_u64
        .saturating_pow(attempt)
        .min(MAX_FALLBACK_DELAY_SECONDS);
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
                id: "event-1".to_owned(),
                subject: Some("Synthetic meeting".to_owned()),
            }
        );
        assert_eq!(
            next[1],
            CalendarChange::Removed {
                id: "event-4".to_owned(),
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
        assert_eq!(
            first[0],
            MessageChange::Upsert {
                id: "message-1".to_owned(),
                subject: Some("Synthetic subject".to_owned()),
            }
        );
        assert_eq!(
            first[1],
            MessageChange::Upsert {
                id: "message-2".to_owned(),
                subject: None,
            }
        );
        assert_eq!(
            next[0],
            MessageChange::Upsert {
                id: "message-3".to_owned(),
                subject: None,
            }
        );
        assert_eq!(actual_cursor, expected_cursor);
    }

    #[test]
    fn graph_links_require_https_authority_and_preserve_the_opaque_query() {
        assert!(matches!(
            GraphAuthority::new("http://graph.microsoft.com/v1.0/"),
            Err(ProviderError::InvalidUrl)
        ));

        let authority = GraphAuthority::new("https://graph.microsoft.com/v1.0/")
            .expect("synthetic HTTPS Graph authority is valid");
        let origin_only = GraphAuthority::new("https://graph.microsoft.com")
            .expect("Graph origin receives the v1.0 prefix");
        assert_eq!(
            origin_only
                .initial_path("me/mailFolders")
                .expect("initial Graph path is valid")
                .as_str(),
            "https://graph.microsoft.com/v1.0/me/mailFolders"
        );
        assert!(matches!(
            GraphAuthority::new("https://graph.microsoft.com/beta/"),
            Err(ProviderError::InvalidUrl)
        ));
        let raw =
            "https://graph.microsoft.com/v1.0/me/messages/delta?$deltatoken=opaque%2Fvalue%3D";
        let accepted = authority
            .opaque_link(raw)
            .expect("same-authority delta cursor is accepted");

        assert_eq!(accepted.as_str(), raw);
        assert!(matches!(
            authority.opaque_link("http://graph.microsoft.com/v1.0/me/messages"),
            Err(ProviderError::UnsafeLink)
        ));
        assert!(matches!(
            authority.opaque_link("https://attacker.example/v1.0/me/messages"),
            Err(ProviderError::UnsafeLink)
        ));
        assert!(matches!(
            authority.opaque_link("/v1.0/me/messages"),
            Err(ProviderError::UnsafeLink)
        ));
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
    fn folder_pages_follow_the_opaque_link_and_parse_synthetic_records() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let next_link = format!(
                "{}/v1.0/me/mailFolders?page=2&$skiptoken=opaque%2Fpage%3D",
                server.base_url()
            );
            let first_body = serde_json::json!({
                "value": [{ "id": "folder-1", "displayName": "Inbox" }],
                "@odata.nextLink": next_link,
            })
            .to_string();
            let second_body = serde_json::json!({
                "value": [{ "id": "folder-2", "displayName": "Archive" }],
            })
            .to_string();

            let first_page = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/mailFolders")
                        .header("Authorization", "Bearer synthetic-test-token")
                        .is_true(|request| request.query_params().is_empty());
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(first_body);
                })
                .await;
            let second_page = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/mailFolders")
                        .query_param("page", "2")
                        .query_param("$skiptoken", "opaque/page=")
                        .header("Authorization", "Bearer synthetic-test-token");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(second_body);
                })
                .await;

            let provider = GraphProvider::start(loopback_authority(&server))
                .await
                .expect("provider worker starts");
            let folders = provider
                .folders(&BearerToken::synthetic())
                .await
                .expect("synthetic folder pages parse");

            assert_eq!(
                folders
                    .iter()
                    .map(|folder| folder.id.as_str())
                    .collect::<Vec<_>>(),
                ["folder-1", "folder-2"]
            );
            assert_eq!(folders[0].display_name, "Inbox");
            assert_eq!(folders[1].display_name, "Archive");
            first_page.assert_async().await;
            second_page.assert_async().await;
            provider.shutdown().await.expect("provider shuts down");
        });
    }

    #[test]
    fn delta_rounds_keep_immutable_ids_and_reuse_the_opaque_delta_link() {
        smol::block_on(async {
            let server = MockServer::start_async().await;
            let next_link = format!(
                "{}/v1.0/me/messages/delta?page=2&$skiptoken=opaque%2Fnext",
                server.base_url()
            );
            let first_cursor = format!(
                "{}/v1.0/me/messages/delta?phase=cursor&$deltatoken=opaque%2Fone",
                server.base_url()
            );
            let second_cursor = format!(
                "{}/v1.0/me/messages/delta?phase=cursor&$deltatoken=opaque%2Ftwo",
                server.base_url()
            );

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
                        .path("/v1.0/me/messages/delta")
                        .query_param("phase", "initial")
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(first_body);
                })
                .await;
            let second_page = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/messages/delta")
                        .query_param("page", "2")
                        .query_param("$skiptoken", "opaque/next")
                        .header("Prefer", "IdType=\"ImmutableId\"");
                    then.status(200)
                        .header("content-type", "application/json")
                        .body(second_body);
                })
                .await;
            let next_round = server
                .mock_async(move |when, then| {
                    when.method("GET")
                        .path("/v1.0/me/messages/delta")
                        .query_param("phase", "cursor")
                        .query_param("$deltatoken", "opaque/one")
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
                .initial_path("me/messages/delta?phase=initial")
                .expect("synthetic delta path is valid");
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
            let delta_link = format!(
                "{}/v1.0/me/messages/delta?$deltatoken=opaque%2Fdeleted",
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
                        .path("/v1.0/me/messages/delta")
                        .query_param("phase", "removals")
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
                .initial_path("me/messages/delta?phase=removals")
                .expect("synthetic delta path is valid");
            let changes = provider
                .message_delta(start, &BearerToken::synthetic())
                .await
                .expect("removed message records parse");

            assert_eq!(changes.records.len(), 2);
            assert_eq!(
                changes.records[0],
                MessageChange::Removed {
                    id: "message-removed".to_owned(),
                    reason: Some("deleted".to_owned()),
                }
            );
            assert_eq!(
                changes.records[1],
                MessageChange::Removed {
                    id: "message-removed-no-reason".to_owned(),
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
                provider.folders(&BearerToken::synthetic()).await,
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
                provider.folders(&BearerToken::synthetic()).await,
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
                provider.folders(&BearerToken::synthetic()).await,
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
    fn retry_after_seconds_http_dates_and_fallback_backoff_are_parsed() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let seconds = HeaderValue::from_static("7");
        let retry_at = now + Duration::from_secs(5);
        let date = httpdate::fmt_http_date(retry_at);
        let date = HeaderValue::from_bytes(date.as_bytes())
            .expect("formatted HTTP date is a valid header");

        assert_eq!(
            retry_after_delay(Some(&seconds), now, 0),
            Duration::from_secs(7)
        );
        assert_eq!(
            retry_after_delay(Some(&date), now, 0),
            Duration::from_secs(5)
        );
        assert_eq!(retry_after_delay(None, now, 0), Duration::from_secs(1));
        assert_eq!(retry_after_delay(None, now, 5), Duration::from_secs(32));
        assert_eq!(retry_after_delay(None, now, 99), Duration::from_mins(1));
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
                    .folders(&BearerToken::synthetic())
                    .await
                    .expect("retry after a synthetic 429 succeeds"),
                Vec::<Folder>::new()
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
                provider.folders(&BearerToken::synthetic()).await,
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
}
