use std::{
    collections::{HashSet, VecDeque},
    str::FromStr,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use addr_spec::AddrSpec;
use einfach_mail_application::{
    ExchangeEwsDiscoveryConfirmation, ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult,
};
use hickory_resolver::{Resolver, lookup::Lookup, proto::rr::RData};
use quick_xml::{
    NsReader,
    events::{BytesStart, Event},
    name::ResolveResult,
};
use reqwest::{Client, Response, StatusCode, header::LOCATION, redirect::Policy};
use tokio::time::timeout;
use url::Url;

use crate::{error::EwsError, transport::parse_endpoint};

const MAX_MAILBOX_ADDRESS_BYTES: usize = 2 * 1024;
const MAX_REQUEST_BYTES: usize = 8 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_XML_DEPTH: usize = 64;
const MAX_SRV_RECORDS: usize = 8;
const MAX_REQUESTS: usize = 12;
const MAX_REDIRECTS: usize = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const DNS_TIMEOUT: Duration = Duration::from_secs(3);
const AUTODISCOVER_PATH: &str = "/autodiscover/autodiscover.xml";
const REQUEST_NAMESPACE: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006";
const RESPONSE_NAMESPACE: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006";
const ACCEPTABLE_RESPONSE_SCHEMA: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a";

#[derive(Default)]
pub(crate) struct EwsAutodiscover {
    pending: Mutex<Option<PendingDiscovery>>,
    next_challenge: AtomicU64,
}

struct PendingDiscovery {
    id: u64,
    state: DiscoveryState,
}

struct DiscoveryState {
    mailbox_address: String,
    candidates: VecDeque<Url>,
    approved_origins: HashSet<String>,
    visited_urls: HashSet<String>,
    visited_mailboxes: HashSet<String>,
    request_count: usize,
    redirect_count: usize,
    explicit_confirmation_required: bool,
}

impl DiscoveryState {
    fn new(mailbox_address: String, candidates: VecDeque<Url>) -> Self {
        let approved_origins = candidates.iter().take(2).map(origin).collect();
        let visited_mailboxes = HashSet::from([mailbox_address.to_ascii_lowercase()]);
        Self {
            mailbox_address,
            candidates,
            approved_origins,
            visited_urls: HashSet::new(),
            visited_mailboxes,
            request_count: 0,
            redirect_count: 0,
            explicit_confirmation_required: false,
        }
    }

    fn needs_confirmation(&self, candidate: &Url) -> bool {
        self.explicit_confirmation_required || !self.approved_origins.contains(&origin(candidate))
    }
}

enum PoxResponse {
    Endpoint(Url),
    RedirectUrl(Url),
    RedirectAddress(String),
    NoEndpoint,
}

enum CandidateAction {
    Exhausted,
    ConfirmationRequired,
    Request(Url),
}

enum ProtocolText {
    Type,
    EwsUrl,
}

enum PoxTextTarget {
    RedirectUrl,
    RedirectAddress,
    Protocol(ProtocolText),
}

/// Returns a result when discovery pauses or completes; `None` means try another candidate.
type DiscoveryStep = Result<Option<ExchangeEwsDiscoveryResult>, EwsError>;

#[derive(Default)]
struct ProtocolEndpoint {
    protocol_type: Option<String>,
    ews_url: Option<String>,
}

struct SrvCandidate {
    priority: u16,
    weight: u16,
    host: String,
    url: Url,
}

#[derive(Default)]
struct PoxParser {
    stack: Vec<String>,
    root_namespace: Option<Vec<u8>>,
    root_closed: bool,
    redirect_url: Option<String>,
    redirect_address: Option<String>,
    protocols: Vec<ProtocolEndpoint>,
    current_protocol: Option<(usize, ProtocolEndpoint)>,
    text_buffer: Option<(PoxTextTarget, String)>,
}

impl EwsAutodiscover {
    pub(crate) async fn discover(
        &self,
        request: ExchangeEwsDiscoveryRequest,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        match request {
            ExchangeEwsDiscoveryRequest::Start { mailbox_address } => {
                self.start_discovery(mailbox_address).await
            }
            ExchangeEwsDiscoveryRequest::Continue(confirmation) => {
                self.continue_discovery(confirmation).await
            }
            ExchangeEwsDiscoveryRequest::Cancel(confirmation) => {
                self.cancel_pending(confirmation.challenge())?;
                Ok(ExchangeEwsDiscoveryResult::Cancelled)
            }
            _ => Err(EwsError::AutodiscoverFailed),
        }
    }

    async fn start_discovery(
        &self,
        mailbox_address: String,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        self.clear_pending()?;
        let mailbox = parse_mailbox(&mailbox_address)?;
        let domain = normalize_domain(mailbox.domain())?;
        let mailbox_address = mailbox.to_string();
        let client = build_client()?;
        timeout(
            DISCOVERY_TIMEOUT,
            self.start_with_client(&client, mailbox_address, domain),
        )
        .await
        .map_err(|_| EwsError::AutodiscoverFailed)?
    }

    async fn start_with_client(
        &self,
        client: &Client,
        mailbox_address: String,
        domain: String,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        let candidates = discovery_candidates(&domain).await;
        let state = DiscoveryState::new(mailbox_address, candidates);
        self.try_candidates(client, state).await
    }

    async fn continue_discovery(
        &self,
        confirmation: ExchangeEwsDiscoveryConfirmation,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        let client = build_client()?;
        timeout(
            DISCOVERY_TIMEOUT,
            self.continue_with_client(&client, confirmation),
        )
        .await
        .map_err(|_| EwsError::AutodiscoverFailed)?
    }

    async fn continue_with_client(
        &self,
        client: &Client,
        confirmation: ExchangeEwsDiscoveryConfirmation,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        let mut pending = self.take_pending(confirmation.challenge())?;
        if let Some(url) = pending.state.candidates.front() {
            pending.state.approved_origins.insert(origin(url));
        }
        pending.state.explicit_confirmation_required = false;
        self.try_candidates(client, pending.state).await
    }

    async fn try_candidates(
        &self,
        client: &Client,
        mut state: DiscoveryState,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        loop {
            match next_candidate_action(&mut state)? {
                CandidateAction::Exhausted => return Err(EwsError::AutodiscoverFailed),
                CandidateAction::ConfirmationRequired => return self.require_confirmation(state),
                CandidateAction::Request(candidate) => {
                    if let Some(discovery_result) =
                        self.try_candidate(client, &mut state, candidate).await?
                    {
                        return Ok(discovery_result);
                    }
                }
            }
        }
    }

    async fn try_candidate(
        &self,
        client: &Client,
        state: &mut DiscoveryState,
        candidate: Url,
    ) -> DiscoveryStep {
        let response = match post_autodiscover(client, &candidate, &state.mailbox_address).await {
            Ok(response) => response,
            Err(EwsError::ServiceUnavailable) => return Ok(None),
            Err(error) => return Err(error),
        };
        if response.status() == StatusCode::OK {
            self.handle_pox_response(response, state).await
        } else if is_redirect_status(response.status()) {
            enqueue_http_redirect(state, &candidate, response)?;
            Ok(None)
        } else {
            Ok(None)
        }
    }

    async fn handle_pox_response(
        &self,
        response: Response,
        state: &mut DiscoveryState,
    ) -> DiscoveryStep {
        let document = match read_limited_body(response).await {
            Ok(document) => document,
            Err(EwsError::ServiceUnavailable) => return Ok(None),
            Err(error) => return Err(error),
        };
        self.apply_pox_response(state, parse_pox_response(&document)?)
            .await
    }

    async fn apply_pox_response(
        &self,
        state: &mut DiscoveryState,
        response: PoxResponse,
    ) -> DiscoveryStep {
        match response {
            PoxResponse::Endpoint(endpoint) => Ok(Some(ExchangeEwsDiscoveryResult::Endpoint(
                endpoint.to_string(),
            ))),
            PoxResponse::RedirectUrl(url) => enqueue_redirect(state, url).map(|()| None),
            PoxResponse::RedirectAddress(address) => {
                count_redirect(state)?;
                self.follow_redirect_address(state, address).await?;
                Ok(None)
            }
            PoxResponse::NoEndpoint => Ok(None),
        }
    }

    async fn follow_redirect_address(
        &self,
        state: &mut DiscoveryState,
        address: String,
    ) -> Result<(), EwsError> {
        let address = parse_mailbox(&address)?;
        let mailbox_address = address.to_string();
        if !state
            .visited_mailboxes
            .insert(mailbox_address.to_ascii_lowercase())
        {
            return Err(EwsError::AutodiscoverFailed);
        }
        let domain = normalize_domain(address.domain())?;
        state.mailbox_address = mailbox_address;
        state.candidates = discovery_candidates(&domain).await;
        state.approved_origins = state.candidates.iter().take(2).map(origin).collect();
        state.explicit_confirmation_required = true;
        Ok(())
    }

    fn require_confirmation(
        &self,
        state: DiscoveryState,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        let confirmation = self.new_confirmation(&state)?;
        self.store_pending(confirmation.challenge(), state)?;
        Ok(ExchangeEwsDiscoveryResult::ConfirmationRequired(
            confirmation,
        ))
    }

    fn new_confirmation(
        &self,
        state: &DiscoveryState,
    ) -> Result<ExchangeEwsDiscoveryConfirmation, EwsError> {
        let destination = state
            .candidates
            .front()
            .map(|url| url.as_str().to_owned())
            .ok_or(EwsError::AutodiscoverFailed)?;
        let challenge = self.next_challenge()?;
        Ok(ExchangeEwsDiscoveryConfirmation::new(
            challenge,
            state.mailbox_address.clone(),
            destination,
        ))
    }

    fn next_challenge(&self) -> Result<u64, EwsError> {
        let mut current = self.next_challenge.load(Ordering::Relaxed);
        loop {
            let next = current.checked_add(1).ok_or(EwsError::AutodiscoverFailed)?;
            match self.next_challenge.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(next),
                Err(observed) => current = observed,
            }
        }
    }

    fn store_pending(&self, id: u64, state: DiscoveryState) -> Result<(), EwsError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| EwsError::ServiceUnavailable)?;
        *pending = Some(PendingDiscovery { id, state });
        Ok(())
    }

    fn take_pending(&self, challenge: u64) -> Result<PendingDiscovery, EwsError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| EwsError::ServiceUnavailable)?;
        if pending
            .as_ref()
            .is_some_and(|pending_discovery| pending_discovery.id == challenge)
        {
            pending.take().ok_or(EwsError::AutodiscoverFailed)
        } else {
            Err(EwsError::AutodiscoverFailed)
        }
    }

    fn cancel_pending(&self, challenge: u64) -> Result<(), EwsError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| EwsError::ServiceUnavailable)?;
        if pending
            .as_ref()
            .is_some_and(|pending_discovery| pending_discovery.id == challenge)
        {
            *pending = None;
        }
        Ok(())
    }

    fn clear_pending(&self) -> Result<(), EwsError> {
        *self
            .pending
            .lock()
            .map_err(|_| EwsError::ServiceUnavailable)? = None;
        Ok(())
    }
}

fn next_candidate_action(state: &mut DiscoveryState) -> Result<CandidateAction, EwsError> {
    while let Some(candidate) = state.candidates.pop_front() {
        if state.visited_urls.contains(candidate.as_str()) {
            continue;
        }
        if state.request_count >= MAX_REQUESTS {
            return Err(EwsError::AutodiscoverFailed);
        }
        if state.needs_confirmation(&candidate) {
            state.candidates.push_front(candidate);
            return Ok(CandidateAction::ConfirmationRequired);
        }
        state.visited_urls.insert(candidate.to_string());
        state.request_count += 1;
        return Ok(CandidateAction::Request(candidate));
    }
    Ok(CandidateAction::Exhausted)
}

fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn build_client() -> Result<Client, EwsError> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .http1_only()
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| EwsError::ServiceUnavailable)
}

async fn post_autodiscover(
    client: &Client,
    endpoint: &Url,
    mailbox_address: &str,
) -> Result<Response, EwsError> {
    let document = request_document(mailbox_address)?;
    client
        .post(endpoint.clone())
        .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=utf-8")
        .body(document)
        .send()
        .await
        .map_err(|_| EwsError::ServiceUnavailable)
}

fn request_document(mailbox_address: &str) -> Result<String, EwsError> {
    if mailbox_address.is_empty() || mailbox_address.len() > MAX_MAILBOX_ADDRESS_BYTES {
        return Err(EwsError::InvalidConnectionDetails);
    }
    let escaped_address = quick_xml::escape::escape(mailbox_address);
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><Autodiscover xmlns=\"{REQUEST_NAMESPACE}\"><Request><EMailAddress>{escaped_address}</EMailAddress><AcceptableResponseSchema>{ACCEPTABLE_RESPONSE_SCHEMA}</AcceptableResponseSchema></Request></Autodiscover>"
    );
    if document.len() > MAX_REQUEST_BYTES {
        return Err(EwsError::ResourceLimitExceeded);
    }
    Ok(document)
}

async fn read_limited_body(mut response: Response) -> Result<Vec<u8>, EwsError> {
    validate_content_length(response.content_length())?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| EwsError::ServiceUnavailable)?
    {
        append_response_chunk(&mut body, &chunk)?;
    }
    Ok(body)
}

fn validate_content_length(content_length: Option<u64>) -> Result<(), EwsError> {
    if content_length.is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(EwsError::ResourceLimitExceeded);
    }
    Ok(())
}

fn append_response_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), EwsError> {
    let new_len = body
        .len()
        .checked_add(chunk.len())
        .ok_or(EwsError::ResourceLimitExceeded)?;
    if new_len > MAX_RESPONSE_BYTES {
        return Err(EwsError::ResourceLimitExceeded);
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn parse_pox_response(document: &[u8]) -> Result<PoxResponse, EwsError> {
    PoxParser::parse(document)
}

impl PoxParser {
    fn parse(document: &[u8]) -> Result<PoxResponse, EwsError> {
        let mut reader = NsReader::from_reader(document);
        reader.config_mut().trim_text(false);
        let decoder = reader.decoder();
        let mut parser = Self::default();

        loop {
            let (namespace, event) = reader
                .read_resolved_event()
                .map_err(|_| EwsError::AutodiscoverFailed)?;
            if matches!(event, Event::Eof) {
                break;
            }
            parser.accept(namespace, event, decoder)?;
        }
        parser.finish()
    }

    fn accept(
        &mut self,
        namespace: ResolveResult<'_>,
        event: Event<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        match event {
            Event::Start(element) => self.start(namespace, &element, decoder),
            Event::Empty(element) => self.empty(namespace, &element, decoder),
            Event::Text(text) => self.text(&text),
            Event::CData(text) => self.cdata(&text),
            Event::End(element) => {
                self.validate_namespace(namespace)?;
                self.end(&element)
            }
            Event::DocType(_) => Err(EwsError::AutodiscoverFailed),
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::Eof => Ok(()),
            Event::GeneralRef(reference) => self.general_reference(&reference),
        }
    }

    fn start(
        &mut self,
        namespace: ResolveResult<'_>,
        element: &BytesStart<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        if self.text_buffer.is_some() {
            return Err(EwsError::AutodiscoverFailed);
        }
        let name = local_name(element);
        if self.stack.is_empty() && name != "Autodiscover" {
            return Err(EwsError::AutodiscoverFailed);
        }
        self.validate_start(namespace)?;
        self.begin_protocol(&name, element, decoder)?;
        self.stack.push(name);
        self.begin_text();
        Ok(())
    }

    fn validate_start(&mut self, namespace: ResolveResult<'_>) -> Result<(), EwsError> {
        if self.root_closed {
            return Err(EwsError::AutodiscoverFailed);
        }
        if self.stack.len() >= MAX_XML_DEPTH {
            return Err(EwsError::ResourceLimitExceeded);
        }
        if self.stack.is_empty() {
            self.set_root_namespace(namespace)
        } else {
            self.validate_namespace(namespace)
        }
    }

    fn validate_namespace(&self, namespace: ResolveResult<'_>) -> Result<(), EwsError> {
        if namespace_matches(namespace, self.root_namespace.as_deref()) {
            Ok(())
        } else {
            Err(EwsError::AutodiscoverFailed)
        }
    }

    fn set_root_namespace(&mut self, namespace: ResolveResult<'_>) -> Result<(), EwsError> {
        if self.root_closed {
            return Err(EwsError::AutodiscoverFailed);
        }
        match namespace {
            ResolveResult::Bound(namespace)
                if namespace.as_ref() == RESPONSE_NAMESPACE.as_bytes() =>
            {
                self.root_namespace = Some(namespace.as_ref().to_vec());
                Ok(())
            }
            _ => Err(EwsError::AutodiscoverFailed),
        }
    }

    fn begin_protocol(
        &mut self,
        name: &str,
        element: &BytesStart<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        if name == "Protocol" && path_is(&self.stack, &["Autodiscover", "Response", "Account"]) {
            self.current_protocol =
                Some((self.stack.len() + 1, protocol_from_start(element, decoder)?));
        }
        Ok(())
    }

    fn empty(
        &mut self,
        namespace: ResolveResult<'_>,
        element: &BytesStart<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        if self.text_buffer.is_some() {
            return Err(EwsError::AutodiscoverFailed);
        }
        if self.stack.is_empty() || !namespace_matches(namespace, self.root_namespace.as_deref()) {
            return Err(EwsError::AutodiscoverFailed);
        }
        if self.stack.len() >= MAX_XML_DEPTH {
            return Err(EwsError::ResourceLimitExceeded);
        }
        self.add_empty_protocol(element, decoder)
    }

    fn add_empty_protocol(
        &mut self,
        element: &BytesStart<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        if local_name(element) == "Protocol"
            && path_is(&self.stack, &["Autodiscover", "Response", "Account"])
        {
            self.protocols.push(protocol_from_start(element, decoder)?);
        }
        Ok(())
    }

    fn text(&mut self, text: &quick_xml::events::BytesText<'_>) -> Result<(), EwsError> {
        let decoded = text.decode().map_err(|_| EwsError::AutodiscoverFailed)?;
        let unescaped =
            quick_xml::escape::unescape(&decoded).map_err(|_| EwsError::AutodiscoverFailed)?;
        self.append_text(&unescaped)
    }

    fn general_reference(
        &mut self,
        reference: &quick_xml::events::BytesRef<'_>,
    ) -> Result<(), EwsError> {
        if self.stack.is_empty() {
            return Err(EwsError::AutodiscoverFailed);
        }
        let reference_text = decode_xml_reference(reference)?;
        self.append_text(&reference_text)
    }

    fn append_text(&mut self, text_chunk: &str) -> Result<(), EwsError> {
        if let Some((_, text)) = &mut self.text_buffer {
            text.push_str(text_chunk);
            return Ok(());
        }
        if self.stack.is_empty() && !text_chunk.trim().is_empty() {
            return Err(EwsError::AutodiscoverFailed);
        }
        Ok(())
    }

    fn cdata(&mut self, text: &quick_xml::events::BytesCData<'_>) -> Result<(), EwsError> {
        if self.stack.is_empty() {
            return Err(EwsError::AutodiscoverFailed);
        }
        let decoded_text = text.decode().map_err(|_| EwsError::AutodiscoverFailed)?;
        self.append_text(&decoded_text)
    }

    fn end(&mut self, element: &quick_xml::events::BytesEnd<'_>) -> Result<(), EwsError> {
        let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
        self.finish_text()?;
        if self.stack.pop().as_deref() != Some(name.as_str()) {
            return Err(EwsError::AutodiscoverFailed);
        }
        self.finish_protocol();
        if self.stack.is_empty() {
            self.root_closed = true;
        }
        Ok(())
    }

    fn begin_text(&mut self) {
        if let Some(target) = pox_text_target(&self.stack) {
            self.text_buffer = Some((target, String::new()));
        }
    }

    fn finish_text(&mut self) -> Result<(), EwsError> {
        let Some((target, text)) = self.text_buffer.take() else {
            return Ok(());
        };
        let field_text = text.trim().to_owned();
        if field_text.is_empty() {
            return Ok(());
        }
        match target {
            PoxTextTarget::RedirectUrl => set_once(&mut self.redirect_url, field_text),
            PoxTextTarget::RedirectAddress => set_once(&mut self.redirect_address, field_text),
            PoxTextTarget::Protocol(ProtocolText::Type) => self.set_protocol_type(field_text),
            PoxTextTarget::Protocol(ProtocolText::EwsUrl) => self.set_protocol_ews_url(field_text),
        }
    }

    fn set_protocol_type(&mut self, protocol_type: String) -> Result<(), EwsError> {
        let Some((depth, protocol)) = &mut self.current_protocol else {
            return Ok(());
        };
        if *depth != self.stack.len() - 1 {
            return Ok(());
        }
        set_once(&mut protocol.protocol_type, protocol_type)
    }

    fn set_protocol_ews_url(&mut self, endpoint: String) -> Result<(), EwsError> {
        let Some((depth, protocol)) = &mut self.current_protocol else {
            return Ok(());
        };
        if *depth != self.stack.len() - 1 {
            return Ok(());
        }
        set_once(&mut protocol.ews_url, endpoint)
    }

    fn finish_protocol(&mut self) {
        if self
            .current_protocol
            .as_ref()
            .is_some_and(|(depth, _)| *depth == self.stack.len() + 1)
            && let Some((_, protocol)) = self.current_protocol.take()
        {
            self.protocols.push(protocol);
        }
    }

    fn finish(self) -> Result<PoxResponse, EwsError> {
        if !self.root_closed || !self.stack.is_empty() {
            return Err(EwsError::AutodiscoverFailed);
        }
        if let Some(address) = self.redirect_address {
            return Ok(PoxResponse::RedirectAddress(address));
        }
        if let Some(url) = self.redirect_url {
            return Ok(PoxResponse::RedirectUrl(parse_endpoint(&url)?));
        }
        self.endpoint()
    }

    fn endpoint(&self) -> Result<PoxResponse, EwsError> {
        for preferred_type in ["EXPR", "EXCH"] {
            for protocol in &self.protocols {
                if protocol.protocol_type.as_deref() == Some(preferred_type)
                    && let Some(endpoint) = &protocol.ews_url
                {
                    return Ok(PoxResponse::Endpoint(parse_endpoint(endpoint)?));
                }
            }
        }
        Ok(PoxResponse::NoEndpoint)
    }
}

fn local_name(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(element.local_name().as_ref()).into_owned()
}

fn pox_text_target(path: &[String]) -> Option<PoxTextTarget> {
    if path_is(
        path,
        &["Autodiscover", "Response", "Account", "RedirectUrl"],
    ) {
        Some(PoxTextTarget::RedirectUrl)
    } else if path_is(
        path,
        &["Autodiscover", "Response", "Account", "RedirectAddr"],
    ) {
        Some(PoxTextTarget::RedirectAddress)
    } else {
        protocol_text_kind(path).map(PoxTextTarget::Protocol)
    }
}

fn namespace_matches(namespace: ResolveResult<'_>, expected: Option<&[u8]>) -> bool {
    matches!(namespace, ResolveResult::Bound(bound_namespace) if Some(bound_namespace.as_ref()) == expected)
}

fn path_is(path: &[String], expected: &[&str]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual == expected)
}

fn protocol_text_kind(path: &[String]) -> Option<ProtocolText> {
    if path_is(
        path,
        &["Autodiscover", "Response", "Account", "Protocol", "Type"],
    ) {
        Some(ProtocolText::Type)
    } else if path_is(
        path,
        &["Autodiscover", "Response", "Account", "Protocol", "ASUrl"],
    ) || path_is(
        path,
        &["Autodiscover", "Response", "Account", "Protocol", "EwsUrl"],
    ) {
        Some(ProtocolText::EwsUrl)
    } else {
        None
    }
}

fn is_xml_character(character: char) -> bool {
    matches!(
        character as u32,
        0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF
    )
}

fn decode_xml_reference(reference: &quick_xml::events::BytesRef<'_>) -> Result<String, EwsError> {
    let character = reference
        .resolve_char_ref()
        .map_err(|_| EwsError::AutodiscoverFailed)?;
    if let Some(character) = character {
        return is_xml_character(character)
            .then(|| character.to_string())
            .ok_or(EwsError::AutodiscoverFailed);
    }
    let name = reference
        .decode()
        .map_err(|_| EwsError::AutodiscoverFailed)?;
    quick_xml::escape::resolve_xml_entity(&name)
        .map(str::to_owned)
        .ok_or(EwsError::AutodiscoverFailed)
}

fn protocol_from_start(
    element: &BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
) -> Result<ProtocolEndpoint, EwsError> {
    let mut protocol = ProtocolEndpoint::default();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| EwsError::AutodiscoverFailed)?;
        if attribute.key.local_name().as_ref() == b"Type" {
            let protocol_type = attribute
                .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, decoder)
                .map_err(|_| EwsError::AutodiscoverFailed)?
                .into_owned();
            set_once(&mut protocol.protocol_type, protocol_type)?;
        }
    }
    Ok(protocol)
}

fn set_once(slot: &mut Option<String>, new_value: String) -> Result<(), EwsError> {
    if slot.as_ref().is_some_and(|current| current != &new_value) {
        return Err(EwsError::AutodiscoverFailed);
    }
    *slot = Some(new_value);
    Ok(())
}

fn parse_mailbox(mailbox: &str) -> Result<AddrSpec, EwsError> {
    if mailbox.len() > MAX_MAILBOX_ADDRESS_BYTES {
        return Err(EwsError::InvalidConnectionDetails);
    }
    let address =
        AddrSpec::from_str(mailbox.trim()).map_err(|_| EwsError::InvalidConnectionDetails)?;
    if address.is_literal() {
        return Err(EwsError::InvalidConnectionDetails);
    }
    Ok(address)
}

fn normalize_domain(domain: &str) -> Result<String, EwsError> {
    let url = Url::parse(&format!("https://{domain}/"))
        .map_err(|_| EwsError::InvalidConnectionDetails)?;
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or(EwsError::InvalidConnectionDetails)?;
    match url::Host::parse(host) {
        Ok(url::Host::Domain(domain)) if !domain.ends_with('.') => Ok(domain),
        _ => Err(EwsError::InvalidConnectionDetails),
    }
}

async fn discovery_candidates(domain: &str) -> VecDeque<Url> {
    let mut candidates = domain_candidates(domain);
    candidates.extend(srv_candidates(domain).await);
    candidates
}

fn domain_candidates(domain: &str) -> VecDeque<Url> {
    let mut candidates = VecDeque::new();
    for host in [format!("autodiscover.{domain}"), domain.to_owned()] {
        if let Ok(url) = candidate_url(&host, None) {
            candidates.push_back(url);
        }
    }
    candidates
}

async fn srv_candidates(domain: &str) -> Vec<Url> {
    let Some(records) = lookup_srv_records(domain).await else {
        return Vec::new();
    };
    let candidates = records
        .answers()
        .iter()
        .filter_map(srv_candidate)
        .collect::<Vec<_>>();
    order_srv_candidates(candidates)
}

fn order_srv_candidates(mut candidates: Vec<SrvCandidate>) -> Vec<Url> {
    candidates.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| right.weight.cmp(&left.weight))
            .then_with(|| left.host.cmp(&right.host))
    });
    candidates
        .into_iter()
        .take(MAX_SRV_RECORDS)
        .map(|candidate| candidate.url)
        .collect()
}

async fn lookup_srv_records(domain: &str) -> Option<Lookup> {
    let resolver = Resolver::builder_tokio().ok()?.build().ok()?;
    let query = format!("_autodiscover._tcp.{domain}.");
    timeout(DNS_TIMEOUT, resolver.srv_lookup(query))
        .await
        .ok()?
        .ok()
}

fn srv_candidate(answer: &hickory_resolver::proto::rr::Record) -> Option<SrvCandidate> {
    let RData::SRV(record) = &answer.data else {
        return None;
    };
    let host = record.target.to_utf8();
    let host = host.trim_end_matches('.');
    if host.is_empty() || record.port == 0 {
        return None;
    }
    let url = candidate_url(host, Some(record.port)).ok()?;
    Some(SrvCandidate {
        priority: record.priority,
        weight: record.weight,
        host: host.to_owned(),
        url,
    })
}

fn candidate_url(host: &str, port: Option<u16>) -> Result<Url, EwsError> {
    let mut url = Url::parse(&format!("https://{host}{AUTODISCOVER_PATH}"))
        .map_err(|_| EwsError::AutodiscoverFailed)?;
    if !matches!(url.host(), Some(url::Host::Domain(_))) {
        return Err(EwsError::AutodiscoverFailed);
    }
    if let Some(port) = port {
        url.set_port(Some(port))
            .map_err(|()| EwsError::AutodiscoverFailed)?;
    }
    validate_discovery_url(url)
}

fn parse_redirect_url(base: &Url, location: &str) -> Result<Url, EwsError> {
    let url = base
        .join(location)
        .map_err(|_| EwsError::AutodiscoverFailed)?;
    validate_discovery_url(url)
}

fn validate_discovery_url(url: Url) -> Result<Url, EwsError> {
    if url.as_str().len() > 2 * 1024
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(EwsError::AutodiscoverFailed);
    }
    Ok(url)
}

fn enqueue_http_redirect(
    state: &mut DiscoveryState,
    base: &Url,
    response: Response,
) -> Result<(), EwsError> {
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|header_value| header_value.to_str().ok())
        .ok_or(EwsError::AutodiscoverFailed)?;
    let redirect = parse_redirect_url(base, location)?;
    enqueue_redirect(state, redirect)
}

fn enqueue_redirect(state: &mut DiscoveryState, redirect: Url) -> Result<(), EwsError> {
    count_redirect(state)?;
    state.explicit_confirmation_required = true;
    state.candidates.push_front(redirect);
    Ok(())
}

fn count_redirect(state: &mut DiscoveryState) -> Result<(), EwsError> {
    state.redirect_count = state
        .redirect_count
        .checked_add(1)
        .ok_or(EwsError::AutodiscoverFailed)?;
    if state.redirect_count > MAX_REDIRECTS {
        return Err(EwsError::AutodiscoverFailed);
    }
    Ok(())
}

fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashSet, VecDeque},
        str::FromStr,
        time::Duration,
    };

    use httpmock::MockServer;
    use reqwest::{Certificate, Client, StatusCode, redirect::Policy};
    use url::Url;

    use super::{
        ACCEPTABLE_RESPONSE_SCHEMA, AUTODISCOVER_PATH, DiscoveryState, EwsAutodiscover,
        MAX_SRV_RECORDS, MAX_XML_DEPTH, PoxResponse, REQUEST_NAMESPACE, REQUEST_TIMEOUT,
        RESPONSE_NAMESPACE, SrvCandidate, candidate_url, domain_candidates, order_srv_candidates,
        origin, parse_pox_response, parse_redirect_url, post_autodiscover, request_document,
    };
    use crate::error::EwsError;
    use einfach_mail_application::{
        ExchangeEwsDiscoveryConfirmation, ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult,
    };

    const ENDPOINT_RESPONSE: &[u8] = include_bytes!("fixtures/autodiscover-endpoint-response.xml");
    const EXCH_RESPONSE: &[u8] = include_bytes!("fixtures/autodiscover-exch-response.xml");
    const REDIRECT_URL_RESPONSE: &[u8] =
        include_bytes!("fixtures/autodiscover-redirect-url-response.xml");
    const REDIRECT_ADDRESS_RESPONSE: &[u8] =
        include_bytes!("fixtures/autodiscover-redirect-address-response.xml");

    fn test_discovery_state(candidates: &[Url], approved: &[Url]) -> DiscoveryState {
        DiscoveryState {
            mailbox_address: "ada@example.test".to_owned(),
            candidates: candidates.iter().cloned().collect::<VecDeque<_>>(),
            approved_origins: approved.iter().map(origin).collect::<HashSet<_>>(),
            visited_urls: HashSet::new(),
            visited_mailboxes: HashSet::from(["ada@example.test".to_owned()]),
            request_count: 0,
            redirect_count: 0,
            explicit_confirmation_required: false,
        }
    }

    fn https_test_client() -> Client {
        let certificate =
            Certificate::from_pem(httpmock::server::DEFAULT_CA_CERTIFICATE.as_bytes())
                .expect("httpmock's synthetic CA certificate parses");
        Client::builder()
            .no_proxy()
            .http1_only()
            .connect_timeout(super::CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .add_root_certificate(certificate)
            .build()
            .expect("synthetic HTTPS client builds")
    }

    #[test]
    fn request_xml_escapes_mailbox_text_and_uses_the_pox_schema() {
        let request = request_document("ada&test@example.test")
            .expect("bounded synthetic mailbox address fits the request");

        assert!(request.contains(&format!("xmlns=\"{REQUEST_NAMESPACE}\"")));
        assert!(request.contains(&format!(
            "<AcceptableResponseSchema>{ACCEPTABLE_RESPONSE_SCHEMA}"
        )));
        assert!(request.contains("ada&amp;test@example.test"));
        assert!(!request.contains("ada&test@example.test"));
    }

    #[test]
    fn candidates_try_domain_hosts_before_srv_hosts() {
        let candidates = domain_candidates("example.test");

        assert_eq!(
            candidates.iter().map(Url::as_str).collect::<Vec<_>>(),
            [
                "https://autodiscover.example.test/autodiscover/autodiscover.xml",
                "https://example.test/autodiscover/autodiscover.xml",
            ]
        );
    }

    #[test]
    fn srv_candidates_sort_by_priority_then_weight_and_apply_the_limit() {
        let mut candidates = (0..MAX_SRV_RECORDS + 1)
            .map(|index| {
                let host = format!("host-{index}.example.test");
                SrvCandidate {
                    priority: 20,
                    weight: 0,
                    url: candidate_url(&host, Some(443)).expect("synthetic SRV target parses"),
                    host,
                }
            })
            .collect::<Vec<_>>();
        candidates.extend([
            SrvCandidate {
                priority: 10,
                weight: 1,
                url: candidate_url("low-weight.example.test", Some(443))
                    .expect("synthetic SRV target parses"),
                host: "low-weight.example.test".to_owned(),
            },
            SrvCandidate {
                priority: 10,
                weight: 5,
                url: candidate_url("high-weight.example.test", Some(443))
                    .expect("synthetic SRV target parses"),
                host: "high-weight.example.test".to_owned(),
            },
        ]);

        let ordered = order_srv_candidates(candidates);

        assert_eq!(ordered.len(), MAX_SRV_RECORDS);
        assert_eq!(ordered[0].host_str(), Some("high-weight.example.test"));
        assert_eq!(ordered[1].host_str(), Some("low-weight.example.test"));
        assert!(ordered[2..].iter().all(|candidate| {
            candidate
                .host_str()
                .is_some_and(|host| host.starts_with("host-"))
        }));
    }

    #[test]
    fn endpoint_response_prefers_expr_and_supports_exch_fallback() {
        assert!(matches!(
            parse_pox_response(ENDPOINT_RESPONSE),
            Ok(PoxResponse::Endpoint(endpoint))
                if endpoint.as_str() == "https://ews.example.test/EWS/Exchange.asmx"
        ));
        assert!(matches!(
            parse_pox_response(EXCH_RESPONSE),
            Ok(PoxResponse::Endpoint(endpoint))
                if endpoint.as_str() == "https://ews.example.test/EWS/Exchange.asmx"
        ));
    }

    #[test]
    fn redirect_response_fixtures_parse_as_supported_redirects() {
        assert!(matches!(
            parse_pox_response(REDIRECT_URL_RESPONSE),
            Ok(PoxResponse::RedirectUrl(url))
                if url.as_str() == "https://redirect.example.test/autodiscover/autodiscover.xml"
        ));
        assert!(matches!(
            parse_pox_response(REDIRECT_ADDRESS_RESPONSE),
            Ok(PoxResponse::RedirectAddress(address)) if address == "ada@redirect.example.test"
        ));
    }

    #[test]
    fn response_parser_decodes_xml_references_and_rejects_unknown_entities() {
        let escaped_address = format!(
            "<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response><Account><RedirectAddr>a&amp;b&#64;example.test</RedirectAddr></Account></Response></Autodiscover>"
        );
        let invalid_character = format!(
            "<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response><Account><RedirectAddr>a&#0;@example.test</RedirectAddr></Account></Response></Autodiscover>"
        );
        let unknown_entity = format!(
            "<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response><Account><RedirectAddr>a&unknown;@example.test</RedirectAddr></Account></Response></Autodiscover>"
        );

        assert!(matches!(
            parse_pox_response(escaped_address.as_bytes()),
            Ok(PoxResponse::RedirectAddress(address)) if address == "a&b@example.test"
        ));
        assert!(matches!(
            parse_pox_response(unknown_entity.as_bytes()),
            Err(EwsError::AutodiscoverFailed)
        ));
        assert!(matches!(
            parse_pox_response(invalid_character.as_bytes()),
            Err(EwsError::AutodiscoverFailed)
        ));
    }

    #[test]
    fn response_body_limit_rejects_declared_and_streamed_overflow() {
        assert!(super::validate_content_length(Some(super::MAX_RESPONSE_BYTES as u64)).is_ok());
        assert_eq!(
            super::validate_content_length(Some(super::MAX_RESPONSE_BYTES as u64 + 1)),
            Err(EwsError::ResourceLimitExceeded)
        );
        let mut body = vec![b'x'; super::MAX_RESPONSE_BYTES - 1];

        assert!(super::append_response_chunk(&mut body, b"y").is_ok());
        assert_eq!(
            super::append_response_chunk(&mut body, b"z"),
            Err(EwsError::ResourceLimitExceeded)
        );
        assert_eq!(body.len(), super::MAX_RESPONSE_BYTES);
    }

    #[test]
    fn response_parser_rejects_wrong_namespace_doctype_and_excessive_depth() {
        let wrong_namespace = br#"<Autodiscover xmlns="urn:wrong"><Response/></Autodiscover>"#;
        let malformed =
            format!("<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response></Autodiscover>");
        let trailing_text = format!(
            "<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response/></Autodiscover>unexpected"
        );
        let doctype = format!(
            "<!DOCTYPE Autodiscover><Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response/></Autodiscover>"
        );
        let nested_elements = "<x>".repeat(MAX_XML_DEPTH + 1);
        let closing_elements = "</x>".repeat(MAX_XML_DEPTH + 1);
        let too_deep = format!(
            "<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\">{nested_elements}{closing_elements}</Autodiscover>"
        );

        assert!(matches!(
            parse_pox_response(wrong_namespace),
            Err(EwsError::AutodiscoverFailed)
        ));
        assert!(matches!(
            parse_pox_response(doctype.as_bytes()),
            Err(EwsError::AutodiscoverFailed)
        ));
        assert!(matches!(
            parse_pox_response(malformed.as_bytes()),
            Err(EwsError::AutodiscoverFailed)
        ));
        assert!(matches!(
            parse_pox_response(trailing_text.as_bytes()),
            Err(EwsError::AutodiscoverFailed)
        ));
        assert!(matches!(
            parse_pox_response(too_deep.as_bytes()),
            Err(EwsError::ResourceLimitExceeded)
        ));
    }

    #[test]
    fn redirect_urls_must_be_https_without_credentials_queries_or_fragments() {
        let base = Url::parse("https://example.test/autodiscover/autodiscover.xml")
            .expect("synthetic base URL parses");
        for location in [
            "http://redirect.example.test/autodiscover.xml",
            "https://user:pass@redirect.example.test/autodiscover.xml",
            "https://redirect.example.test/autodiscover.xml?next=private",
            "https://redirect.example.test/autodiscover.xml#fragment",
        ] {
            assert!(parse_redirect_url(&base, location).is_err(), "{location}");
        }
        let long_path = format!("https://redirect.example.test/{}", "a".repeat(2 * 1024));
        assert!(parse_redirect_url(&base, &long_path).is_err());
        assert!(candidate_url("example.test", Some(0)).is_err());
        assert!(candidate_url("127.0.0.1", None).is_err());
        assert!(super::normalize_domain("127.0.0.1").is_err());
    }

    #[tokio::test]
    async fn cross_origin_pox_redirect_waits_for_confirmation_and_sends_no_authentication() {
        let source = MockServer::start_async().await;
        let destination = MockServer::start_async().await;
        let target = destination.url(AUTODISCOVER_PATH);
        let source_response = format!(
            "<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response><Account><RedirectUrl>{target}</RedirectUrl></Account></Response></Autodiscover>"
        );
        let source_request = source
            .mock_async(|when, then| {
                when.method("POST")
                    .path(AUTODISCOVER_PATH)
                    .header_missing("Authorization");
                then.status(StatusCode::OK)
                    .header("Content-Type", "text/xml; charset=utf-8")
                    .body(source_response);
            })
            .await;
        let destination_request = destination
            .mock_async(|when, then| {
                when.method("POST")
                    .path(AUTODISCOVER_PATH)
                    .header_missing("Authorization");
                then.status(StatusCode::OK)
                    .header("Content-Type", "text/xml; charset=utf-8")
                    .body(ENDPOINT_RESPONSE);
            })
            .await;
        let source_url =
            Url::from_str(&source.url(AUTODISCOVER_PATH)).expect("mock origin URL parses");
        let destination_url = Url::from_str(&target).expect("mock destination URL parses");
        let autodiscover = EwsAutodiscover::default();
        let state = test_discovery_state(
            std::slice::from_ref(&source_url),
            std::slice::from_ref(&source_url),
        );
        let client = https_test_client();

        let discovery_result = autodiscover
            .try_candidates(&client, state)
            .await
            .expect("the cross-origin response becomes a confirmation request");
        let ExchangeEwsDiscoveryResult::ConfirmationRequired(confirmation) = discovery_result
        else {
            panic!("cross-origin destination must require confirmation");
        };

        assert_eq!(confirmation.mailbox_address(), "ada@example.test");
        assert_eq!(confirmation.destination(), destination_url.as_str());
        source_request.assert_async().await;
        destination_request.assert_calls(0);

        let continued = autodiscover
            .continue_with_client(&client, confirmation)
            .await
            .expect("confirmed discovery reaches the redirected host");
        assert!(matches!(
            continued,
            ExchangeEwsDiscoveryResult::Endpoint(endpoint)
                if endpoint == "https://ews.example.test/EWS/Exchange.asmx"
        ));
        destination_request.assert_async().await;
    }

    #[tokio::test]
    async fn http_redirect_waits_for_confirmation_even_when_the_origin_is_unchanged() {
        let server = MockServer::start_async().await;
        let redirected_path = "/autodiscover/redirected.xml";
        let redirected_url = server.url(redirected_path);
        let source_request = server
            .mock_async(|when, then| {
                when.method("POST")
                    .path(AUTODISCOVER_PATH)
                    .header_missing("Authorization");
                then.status(StatusCode::FOUND)
                    .header("Location", redirected_url.clone());
            })
            .await;
        let redirected_request = server
            .mock_async(|when, then| {
                when.method("POST")
                    .path(redirected_path)
                    .header_missing("Authorization");
                then.status(StatusCode::OK)
                    .header("Content-Type", "text/xml; charset=utf-8")
                    .body(ENDPOINT_RESPONSE);
            })
            .await;
        let source_url =
            Url::from_str(&server.url(AUTODISCOVER_PATH)).expect("mock origin URL parses");
        let redirect_url = Url::from_str(&redirected_url).expect("redirect URL parses");
        let autodiscover = EwsAutodiscover::default();
        let state = test_discovery_state(
            std::slice::from_ref(&source_url),
            std::slice::from_ref(&source_url),
        );
        let client = https_test_client();

        let discovery_result = autodiscover
            .try_candidates(&client, state)
            .await
            .expect("the redirect becomes a confirmation request");
        let ExchangeEwsDiscoveryResult::ConfirmationRequired(confirmation) = discovery_result
        else {
            panic!("HTTP redirects must require explicit confirmation");
        };

        assert_eq!(confirmation.destination(), redirect_url.as_str());
        assert_eq!(origin(&redirect_url), origin(&source_url));
        source_request.assert_async().await;
        redirected_request.assert_calls(0);

        let continued = autodiscover
            .continue_with_client(&client, confirmation)
            .await
            .expect("confirmed discovery reaches the redirected path");
        assert!(matches!(
            continued,
            ExchangeEwsDiscoveryResult::Endpoint(endpoint)
                if endpoint == "https://ews.example.test/EWS/Exchange.asmx"
        ));
        redirected_request.assert_async().await;
    }

    #[tokio::test]
    async fn autodiscover_redirect_loop_stops_before_repeating_a_request() {
        let server = MockServer::start_async().await;
        let source_url = server.url(AUTODISCOVER_PATH);
        let response = format!(
            "<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response><Account><RedirectUrl>{source_url}</RedirectUrl></Account></Response></Autodiscover>"
        );
        let source_request = server
            .mock_async(|when, then| {
                when.method("POST")
                    .path(AUTODISCOVER_PATH)
                    .header_missing("Authorization");
                then.status(StatusCode::OK)
                    .header("Content-Type", "text/xml; charset=utf-8")
                    .body(response);
            })
            .await;
        let source_url = Url::from_str(&source_url).expect("mock origin URL parses");
        let state = test_discovery_state(
            std::slice::from_ref(&source_url),
            std::slice::from_ref(&source_url),
        );
        let client = https_test_client();

        assert!(matches!(
            EwsAutodiscover::default()
                .try_candidates(&client, state)
                .await,
            Err(EwsError::AutodiscoverFailed)
        ));
        source_request.assert_calls(1);
    }

    #[tokio::test]
    async fn request_timeout_stops_a_slow_autodiscover_server() {
        let server = MockServer::start_async().await;
        let slow_request = server
            .mock_async(|when, then| {
                when.method("POST")
                    .path(AUTODISCOVER_PATH)
                    .header_missing("Authorization");
                then.status(StatusCode::OK)
                    .delay(REQUEST_TIMEOUT + Duration::from_secs(1))
                    .body(ENDPOINT_RESPONSE);
            })
            .await;
        let endpoint =
            Url::from_str(&server.url(AUTODISCOVER_PATH)).expect("mock origin URL parses");
        let client = https_test_client();

        assert!(matches!(
            post_autodiscover(&client, &endpoint, "ada@example.test").await,
            Err(EwsError::ServiceUnavailable)
        ));
        slow_request.assert_async().await;
    }

    #[tokio::test]
    async fn cancelling_redirect_confirmation_does_not_contact_the_destination() {
        let source = MockServer::start_async().await;
        let destination = MockServer::start_async().await;
        let destination_url = destination.url(AUTODISCOVER_PATH);
        let source_response = format!(
            "<Autodiscover xmlns=\"{RESPONSE_NAMESPACE}\"><Response><Account><RedirectUrl>{destination_url}</RedirectUrl></Account></Response></Autodiscover>"
        );
        let source_request = source
            .mock_async(|when, then| {
                when.method("POST")
                    .path(AUTODISCOVER_PATH)
                    .header_missing("Authorization");
                then.status(StatusCode::OK)
                    .header("Content-Type", "text/xml; charset=utf-8")
                    .body(source_response);
            })
            .await;
        let destination_request = destination
            .mock_async(|when, then| {
                when.method("POST")
                    .path(AUTODISCOVER_PATH)
                    .header_missing("Authorization");
                then.status(StatusCode::OK)
                    .header("Content-Type", "text/xml; charset=utf-8")
                    .body(ENDPOINT_RESPONSE);
            })
            .await;
        let source_url =
            Url::from_str(&source.url(AUTODISCOVER_PATH)).expect("mock origin URL parses");
        let state = test_discovery_state(
            std::slice::from_ref(&source_url),
            std::slice::from_ref(&source_url),
        );
        let autodiscover = EwsAutodiscover::default();
        let client = https_test_client();
        let discovery_result = autodiscover
            .try_candidates(&client, state)
            .await
            .expect("the redirect becomes a confirmation request");
        let ExchangeEwsDiscoveryResult::ConfirmationRequired(confirmation) = discovery_result
        else {
            panic!("redirect destination must require explicit confirmation");
        };

        let cancelled = autodiscover
            .discover(ExchangeEwsDiscoveryRequest::Cancel(confirmation.clone()))
            .await
            .expect("cancel removes the pending destination");

        assert!(matches!(cancelled, ExchangeEwsDiscoveryResult::Cancelled));
        source_request.assert_async().await;
        destination_request.assert_calls(0);
        assert!(
            autodiscover
                .continue_with_client(&client, confirmation)
                .await
                .is_err()
        );
        destination_request.assert_calls(0);
    }

    #[test]
    fn confirmation_debug_output_redacts_mailbox_and_destination() {
        let confirmation = ExchangeEwsDiscoveryConfirmation::new(
            41,
            "ada@example.test".to_owned(),
            "https://redirect.example.test".to_owned(),
        );
        let debug_output = format!("{confirmation:?}");

        assert!(!debug_output.contains("ada@example.test"));
        assert!(!debug_output.contains("redirect.example.test"));
        assert!(!debug_output.contains("41"));
    }
}
