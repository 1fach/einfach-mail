use std::{
    net::SocketAddr,
    sync::mpsc as sync_mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use ews::{Operation, OperationResponse, server_version::ExchangeServerVersion, soap::Envelope};
use hyper_util::client::legacy::connect::HttpInfo;
use reqwest::{
    Client, Response, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, WWW_AUTHENTICATE},
    redirect::Policy,
    tls::TlsInfo,
};
use secrecy::SecretString;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};
use tokio::{
    runtime::Builder,
    sync::{mpsc, oneshot},
};
use url::Url;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    error::EwsError,
    ntlm::{NtlmHandshake, decode_challenge, encode_token},
};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SOAP_DEPTH: usize = 128;
const MAX_NTLM_TOKEN_BYTES: usize = 16 * 1024;
const REQUEST_CHANNEL_CAPACITY: usize = 16;

pub(crate) struct EwsTransport {
    commands: mpsc::Sender<EwsCommand>,
    shutdown: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

struct EwsTransportState {
    endpoint: Url,
    target_name: String,
    username: String,
    password: SecretString,
    client: Client,
    authenticated_connection: Option<ConnectionIdentity>,
}

struct EwsCommand {
    document: Vec<u8>,
    response: oneshot::Sender<Result<Vec<u8>, EwsError>>,
}

impl EwsTransport {
    pub(crate) fn new(
        endpoint: &str,
        username: String,
        password: SecretString,
    ) -> Result<Self, EwsError> {
        Self::new_with_client_builder(endpoint, username, password, build_client_builder())
    }

    fn new_with_client_builder(
        endpoint: &str,
        username: String,
        password: SecretString,
        client_builder: reqwest::ClientBuilder,
    ) -> Result<Self, EwsError> {
        let endpoint = parse_endpoint(endpoint)?;
        let target_name = endpoint
            .host_str()
            .filter(|host| !host.is_empty())
            .ok_or(EwsError::InvalidConnectionDetails)?
            .to_owned();

        let (commands, command_receiver) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let (ready_sender, ready_receiver) = sync_mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("einfach-mail-ews-transport".to_owned())
            .spawn(move || {
                let runtime = match Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = ready_sender.send(Err(EwsError::ServiceUnavailable));
                        return;
                    }
                };
                let client = match client_builder.build() {
                    Ok(client) => client,
                    Err(_) => {
                        let _ = ready_sender.send(Err(EwsError::ServiceUnavailable));
                        return;
                    }
                };
                let state = EwsTransportState {
                    endpoint,
                    target_name,
                    username,
                    password,
                    client,
                    authenticated_connection: None,
                };
                if ready_sender.send(Ok(())).is_err() {
                    return;
                }
                runtime.block_on(run_worker(command_receiver, shutdown_receiver, state));
            })
            .map_err(|_| EwsError::ServiceUnavailable)?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                commands,
                shutdown: Some(shutdown),
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(EwsError::ServiceUnavailable)
            }
        }
    }

    pub(crate) async fn post_soap(&self, document: &[u8]) -> Result<Vec<u8>, EwsError> {
        if document.is_empty() || document.len() > MAX_REQUEST_BYTES {
            return Err(EwsError::ResourceLimitExceeded);
        }
        let (response_sender, response_receiver) = oneshot::channel();
        self.commands
            .send(EwsCommand {
                document: document.to_vec(),
                response: response_sender,
            })
            .await
            .map_err(|_| EwsError::ServiceUnavailable)?;
        response_receiver
            .await
            .map_err(|_| EwsError::ServiceUnavailable)?
    }

    pub(crate) async fn execute<O>(
        &self,
        operation: O,
    ) -> Result<Vec<ews::ResponseClass<<O::Response as OperationResponse>::Message>>, EwsError>
    where
        O: Operation,
    {
        self.execute_with_size(operation)
            .await
            .map(|(messages, _)| messages)
    }

    pub(crate) async fn execute_with_size<O>(
        &self,
        operation: O,
    ) -> Result<
        (
            Vec<ews::ResponseClass<<O::Response as OperationResponse>::Message>>,
            usize,
        ),
        EwsError,
    >
    where
        O: Operation,
    {
        let document = serialize_soap_request(operation)?;
        let response = self.post_soap(&document).await?;
        let response_size = response.len();
        Ok((parse_soap_response::<O>(&response)?, response_size))
    }
}

impl Drop for EwsTransport {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl EwsTransportState {
    async fn post_soap(&mut self, document: &[u8]) -> Result<Vec<u8>, EwsError> {
        let first = self.send(document, None).await?;
        if first.status == StatusCode::OK {
            if self.authenticated_connection.as_ref() != Some(&first.identity) {
                self.authenticated_connection = None;
                return Err(EwsError::AccessDenied);
            }
            return Ok(first.body);
        }
        if first.status != StatusCode::UNAUTHORIZED {
            return Err(map_status(first.status));
        }

        let mut handshake = NtlmHandshake::new(&self.username, &self.password)?;
        let initial_token = handshake.initial_token(&self.target_name)?;
        let initial_header = ntlm_header(&initial_token)?;
        let type_two = self.send(document, Some(initial_header)).await?;
        if type_two.status != StatusCode::UNAUTHORIZED {
            return Err(map_status(type_two.status));
        }
        let challenge = ntlm_challenge(&type_two.headers)?;
        let challenge_token = decode_challenge(challenge)?;
        let channel_binding = tls_server_end_point(&type_two.peer_certificate)?;
        let response_token =
            handshake.response_token(&self.target_name, &challenge_token, &channel_binding)?;
        let response_header = ntlm_header(&response_token)?;
        let authenticated = self.send(document, Some(response_header)).await?;
        if authenticated.identity != type_two.identity {
            self.authenticated_connection = None;
            return Err(EwsError::InvalidResponse);
        }
        if authenticated.status != StatusCode::OK {
            self.authenticated_connection = None;
            return Err(map_status(authenticated.status));
        }

        self.authenticated_connection = Some(authenticated.identity);
        Ok(authenticated.body)
    }

    async fn send(
        &self,
        document: &[u8],
        authorization: Option<HeaderValue>,
    ) -> Result<ReceivedResponse, EwsError> {
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .header(CONTENT_TYPE, "text/xml; charset=utf-8")
            .body(document.to_vec());
        if let Some(authorization) = authorization {
            request = request.header(AUTHORIZATION, authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|_| EwsError::ServiceUnavailable)?;
        receive_response(response).await
    }
}

async fn run_worker(
    mut commands: mpsc::Receiver<EwsCommand>,
    mut shutdown: oneshot::Receiver<()>,
    mut state: EwsTransportState,
) {
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                let response = tokio::select! {
                    biased;
                    _ = &mut shutdown => break,
                    response = state.post_soap(&command.document) => response,
                };
                let _ = command.response.send(response);
            }
        }
    }
}

fn build_client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(35))
        .pool_idle_timeout(Duration::from_secs(60))
        .pool_max_idle_per_host(1)
        .http1_only()
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .tls_info(true)
}

fn serialize_soap_request<O: Operation>(operation: O) -> Result<Vec<u8>, EwsError> {
    Envelope {
        headers: vec![ews::soap::Header::RequestServerVersion {
            version: ExchangeServerVersion::Exchange2013_SP1,
        }],
        body: operation,
    }
    .as_xml_document()
    .map_err(|_| EwsError::InvalidResponse)
}

pub(crate) fn parse_soap_response<O: Operation>(
    document: &[u8],
) -> Result<Vec<ews::ResponseClass<<O::Response as OperationResponse>::Message>>, EwsError> {
    validate_soap_header(document)?;
    let envelope = Envelope::<O::Response>::from_xml_document(document)
        .map_err(|_| EwsError::InvalidResponse)?;
    Ok(envelope.body.into_response_messages())
}

struct ReceivedResponse {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    identity: ConnectionIdentity,
    peer_certificate: Vec<u8>,
    body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConnectionIdentity {
    local: SocketAddr,
    remote: SocketAddr,
    certificate_hash: [u8; 32],
}

async fn receive_response(mut response: Response) -> Result<ReceivedResponse, EwsError> {
    let (status, headers, identity, peer_certificate) = {
        let extensions = response.extensions();
        let info = extensions
            .get::<HttpInfo>()
            .ok_or(EwsError::InvalidResponse)?;
        let tls_info = extensions
            .get::<TlsInfo>()
            .ok_or(EwsError::InvalidResponse)?;
        let peer_certificate = tls_info
            .peer_certificate()
            .ok_or(EwsError::InvalidResponse)?
            .to_vec();
        let identity = ConnectionIdentity {
            local: info.local_addr(),
            remote: info.remote_addr(),
            certificate_hash: Sha256::digest(&peer_certificate).into(),
        };
        (
            response.status(),
            response.headers().clone(),
            identity,
            peer_certificate,
        )
    };

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| EwsError::ServiceUnavailable)?
    {
        let new_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(EwsError::ResourceLimitExceeded)?;
        if new_len > MAX_RESPONSE_BYTES {
            return Err(EwsError::ResourceLimitExceeded);
        }
        body.extend_from_slice(&chunk);
    }

    Ok(ReceivedResponse {
        status,
        headers,
        identity,
        peer_certificate,
        body,
    })
}

fn parse_endpoint(endpoint: &str) -> Result<Url, EwsError> {
    if endpoint.is_empty() || endpoint.len() > 2 * 1024 || endpoint.chars().any(char::is_control) {
        return Err(EwsError::InvalidConnectionDetails);
    }
    let parsed = Url::parse(endpoint).map_err(|_| EwsError::InvalidConnectionDetails)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(EwsError::InvalidConnectionDetails);
    }
    Ok(parsed)
}

fn validate_soap_header(document: &[u8]) -> Result<(), EwsError> {
    use quick_xml::{NsReader, events::Event, name::ResolveResult};

    const SOAP_NAMESPACE: &[u8] = b"http://schemas.xmlsoap.org/soap/envelope/";

    let mut reader = NsReader::from_reader(document);
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut header_seen = false;
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| EwsError::InvalidResponse)?;
        match event {
            Event::Start(element) => {
                if depth >= MAX_SOAP_DEPTH {
                    return Err(EwsError::ResourceLimitExceeded);
                }
                if depth == 0 {
                    if root_seen
                        || !matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == SOAP_NAMESPACE)
                        || element.local_name().as_ref() != b"Envelope"
                    {
                        return Err(EwsError::InvalidResponse);
                    }
                    root_seen = true;
                } else if depth == 1
                    && matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == SOAP_NAMESPACE)
                    && element.local_name().as_ref() == b"Header"
                {
                    header_seen = true;
                }
                depth = depth
                    .checked_add(1)
                    .ok_or(EwsError::ResourceLimitExceeded)?;
            }
            Event::Empty(element) => {
                if depth >= MAX_SOAP_DEPTH {
                    return Err(EwsError::ResourceLimitExceeded);
                }
                if depth == 1
                    && matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == SOAP_NAMESPACE)
                    && element.local_name().as_ref() == b"Header"
                {
                    header_seen = true;
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or(EwsError::InvalidResponse)?;
            }
            Event::Eof => {
                return (root_seen && header_seen && depth == 0)
                    .then_some(())
                    .ok_or(EwsError::InvalidResponse);
            }
            _ => {}
        }
    }
}

fn ntlm_header(token: &[u8]) -> Result<HeaderValue, EwsError> {
    if token.is_empty() || token.len() > MAX_NTLM_TOKEN_BYTES {
        return Err(EwsError::ResourceLimitExceeded);
    }
    let encoded = encode_token(token)?;
    let mut value = Zeroizing::new(format!("NTLM {}", encoded.as_str()));
    let mut header = HeaderValue::from_str(&value).map_err(|_| EwsError::InvalidResponse)?;
    header.set_sensitive(true);
    value.zeroize();
    Ok(header)
}

fn ntlm_challenge(headers: &reqwest::header::HeaderMap) -> Result<&str, EwsError> {
    headers
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let (scheme, token) = value.split_once(' ')?;
            (scheme.eq_ignore_ascii_case("NTLM") && !token.trim().is_empty())
                .then_some(token.trim())
        })
        .ok_or(EwsError::AccessDenied)
}

fn map_status(status: StatusCode) -> EwsError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => EwsError::AccessDenied,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT => EwsError::ServiceUnavailable,
        status if status.is_server_error() => EwsError::ServiceUnavailable,
        _ => EwsError::InvalidResponse,
    }
}

fn tls_server_end_point(certificate_der: &[u8]) -> Result<Zeroizing<Vec<u8>>, EwsError> {
    let (_, certificate) = x509_parser::parse_x509_certificate(certificate_der)
        .map_err(|_| EwsError::InvalidResponse)?;
    let signature_algorithm = &certificate.signature_algorithm;
    let signature_oid = signature_algorithm.algorithm.to_id_string();
    let digest = if signature_oid == "1.2.840.113549.1.1.10" {
        use x509_parser::signature_algorithm::SignatureAlgorithm;

        let SignatureAlgorithm::RSASSA_PSS(parameters) =
            SignatureAlgorithm::try_from(signature_algorithm)
                .map_err(|_| EwsError::InvalidResponse)?
        else {
            return Err(EwsError::InvalidResponse);
        };
        digest_for_hash_oid(&parameters.hash_algorithm_oid().to_id_string())?
    } else {
        digest_for_signature_oid(&signature_oid)?
    };
    let mut binding = Zeroizing::new(b"tls-server-end-point:".to_vec());
    match digest {
        DigestAlgorithm::Sha256 => binding.extend_from_slice(&Sha256::digest(certificate_der)),
        DigestAlgorithm::Sha224 => binding.extend_from_slice(&Sha224::digest(certificate_der)),
        DigestAlgorithm::Sha384 => binding.extend_from_slice(&Sha384::digest(certificate_der)),
        DigestAlgorithm::Sha512 => binding.extend_from_slice(&Sha512::digest(certificate_der)),
    }
    Ok(binding)
}

fn digest_for_signature_oid(signature_oid: &str) -> Result<DigestAlgorithm, EwsError> {
    match signature_oid {
        "1.2.840.113549.1.1.4"
        | "1.2.840.113549.1.1.5"
        | "1.2.840.10040.4.3"
        | "1.2.840.10045.4.1" => Ok(DigestAlgorithm::Sha256),
        "1.2.840.113549.1.1.14" | "1.2.840.10045.4.3.1" | "2.16.840.1.101.3.4.3.1" => {
            Ok(DigestAlgorithm::Sha224)
        }
        "1.2.840.113549.1.1.11" | "1.2.840.10045.4.3.2" | "2.16.840.1.101.3.4.3.2" => {
            Ok(DigestAlgorithm::Sha256)
        }
        "1.2.840.113549.1.1.12" | "1.2.840.10045.4.3.3" | "2.16.840.1.101.3.4.3.3" => {
            Ok(DigestAlgorithm::Sha384)
        }
        "1.2.840.113549.1.1.13" | "1.2.840.10045.4.3.4" | "2.16.840.1.101.3.4.3.4" => {
            Ok(DigestAlgorithm::Sha512)
        }
        _ => Err(EwsError::InvalidResponse),
    }
}

fn digest_for_hash_oid(hash_oid: &str) -> Result<DigestAlgorithm, EwsError> {
    match hash_oid {
        "1.3.14.3.2.26" | "2.16.840.1.101.3.4.2.1" => Ok(DigestAlgorithm::Sha256),
        "2.16.840.1.101.3.4.2.4" => Ok(DigestAlgorithm::Sha224),
        "2.16.840.1.101.3.4.2.2" => Ok(DigestAlgorithm::Sha384),
        "2.16.840.1.101.3.4.2.3" => Ok(DigestAlgorithm::Sha512),
        _ => Err(EwsError::InvalidResponse),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DigestAlgorithm {
    Sha256,
    Sha224,
    Sha384,
    Sha512,
}

#[cfg(test)]
mod tests {
    use super::{
        DigestAlgorithm, EwsTransport, MAX_SOAP_DEPTH, build_client_builder, digest_for_hash_oid,
        digest_for_signature_oid, map_status, parse_endpoint, parse_soap_response,
        serialize_soap_request, validate_soap_header,
    };
    use crate::error::EwsError;
    use crate::ntlm::synthetic_type_two_challenge;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use ews::{
        BaseFolderId, BaseShape, ItemShape, PathToElement, ResponseClass,
        sync_folder_items::{SyncFolderItems, SyncScope},
    };
    use httpmock::MockServer;
    use reqwest::{Certificate, StatusCode, header::AUTHORIZATION};
    use secrecy::SecretString;

    const SYNC_FOLDER_ITEMS_PAGE: &[u8] = include_bytes!("fixtures/sync-folder-items-page.xml");

    fn sync_folder_items_request(sync_state: Option<String>) -> SyncFolderItems {
        SyncFolderItems {
            item_shape: ItemShape {
                base_shape: BaseShape::IdOnly,
                include_mime_content: Some(false),
                additional_properties: Some(vec![PathToElement::FieldURI {
                    field_URI: "item:Subject".to_owned(),
                }]),
            },
            sync_folder_id: BaseFolderId::new_folder("synthetic-folder"),
            sync_state,
            ignore: None,
            max_changes_returned: 512,
            sync_scope: Some(SyncScope::NormalItems),
        }
    }

    #[test]
    fn accepts_only_explicit_https_endpoints_without_user_info_or_query() {
        for endpoint in [
            "http://exchange.example.test/EWS/Exchange.asmx",
            "https://user:password@exchange.example.test/EWS/Exchange.asmx",
            "https://exchange.example.test/EWS/Exchange.asmx?tenant=x",
            "https://exchange.example.test/EWS/Exchange.asmx#fragment",
        ] {
            assert_eq!(
                parse_endpoint(endpoint),
                Err(EwsError::InvalidConnectionDetails)
            );
        }
        assert!(parse_endpoint("https://exchange.example.test/EWS/Exchange.asmx").is_ok());
    }

    #[test]
    fn maps_authentication_and_transient_statuses_without_exposing_response_text() {
        assert_eq!(map_status(StatusCode::UNAUTHORIZED), EwsError::AccessDenied);
        assert_eq!(
            map_status(StatusCode::TOO_MANY_REQUESTS),
            EwsError::ServiceUnavailable
        );
        assert_eq!(
            map_status(StatusCode::BAD_REQUEST),
            EwsError::InvalidResponse
        );
    }

    #[test]
    fn selects_the_rfc_5929_digest_for_single_hash_signature_algorithms() {
        for (signature_oid, expected) in [
            ("1.2.840.113549.1.1.5", DigestAlgorithm::Sha256),
            ("1.2.840.113549.1.1.14", DigestAlgorithm::Sha224),
            ("1.2.840.10045.4.3.1", DigestAlgorithm::Sha224),
            ("2.16.840.1.101.3.4.3.2", DigestAlgorithm::Sha256),
            ("1.2.840.10045.4.3.3", DigestAlgorithm::Sha384),
            ("2.16.840.1.101.3.4.3.4", DigestAlgorithm::Sha512),
        ] {
            assert_eq!(digest_for_signature_oid(signature_oid), Ok(expected));
        }
        assert_eq!(
            digest_for_signature_oid("1.3.101.112"),
            Err(EwsError::InvalidResponse)
        );
    }

    #[test]
    fn selects_the_rfc_5929_digest_for_rsa_pss_hash_parameters() {
        for (hash_oid, expected) in [
            ("1.3.14.3.2.26", DigestAlgorithm::Sha256),
            ("2.16.840.1.101.3.4.2.4", DigestAlgorithm::Sha224),
            ("2.16.840.1.101.3.4.2.1", DigestAlgorithm::Sha256),
            ("2.16.840.1.101.3.4.2.2", DigestAlgorithm::Sha384),
            ("2.16.840.1.101.3.4.2.3", DigestAlgorithm::Sha512),
        ] {
            assert_eq!(digest_for_hash_oid(hash_oid), Ok(expected));
        }
        assert_eq!(
            digest_for_hash_oid("1.2.3.4"),
            Err(EwsError::InvalidResponse)
        );
    }

    #[test]
    fn requires_a_soap_header_before_using_the_ews_parser() {
        let with_header = br#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Header/><s:Body/></s:Envelope>"#;
        let without_header = br#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body/></s:Envelope>"#;
        let wrong_namespace =
            br#"<s:Envelope xmlns:s="urn:invalid"><s:Header/><s:Body/></s:Envelope>"#;

        assert_eq!(validate_soap_header(with_header), Ok(()));
        assert_eq!(
            validate_soap_header(without_header),
            Err(EwsError::InvalidResponse)
        );
        assert_eq!(
            validate_soap_header(wrong_namespace),
            Err(EwsError::InvalidResponse)
        );
    }

    #[test]
    fn rejects_soap_documents_that_exceed_the_nesting_limit() {
        let opening_elements = "<x>".repeat(MAX_SOAP_DEPTH);
        let closing_elements = "</x>".repeat(MAX_SOAP_DEPTH);
        let document = format!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Header/><s:Body>{opening_elements}{closing_elements}</s:Body></s:Envelope>"
        );

        assert_eq!(
            validate_soap_header(document.as_bytes()),
            Err(EwsError::ResourceLimitExceeded)
        );
    }

    #[test]
    fn serializes_sync_requests_with_the_selected_cursor_and_bounded_shape() {
        let xml = serialize_soap_request(sync_folder_items_request(Some(
            "synthetic-cursor".to_owned(),
        )))
        .expect("synthetic EWS request serializes");
        let xml = String::from_utf8(xml).expect("synthetic EWS request is UTF-8");

        assert!(xml.contains("RequestServerVersion"));
        assert!(xml.contains("Exchange2013_SP1"));
        assert!(xml.contains("SyncFolderItems"));
        assert!(xml.contains("synthetic-cursor"));
        assert!(xml.contains("MaxChangesReturned"));
        assert!(!xml.contains("synthetic-only"));
    }

    #[test]
    fn parses_a_synthetic_paged_ews_response_fixture() {
        let mut responses = parse_soap_response::<SyncFolderItems>(SYNC_FOLDER_ITEMS_PAGE)
            .expect("synthetic SOAP response parses");

        assert_eq!(responses.len(), 1);
        let ResponseClass::Success(response) = responses.pop().expect("one response exists") else {
            panic!("synthetic page has a successful response class");
        };
        assert_eq!(response.sync_state, "synthetic-page-cursor");
        assert!(!response.includes_last_item_in_range);
        assert_eq!(response.changes.inner.len(), 1);
    }

    #[tokio::test]
    async fn sends_ews_soap_over_https_with_a_serialized_ntlmv2_handshake() {
        let server = MockServer::start_async().await;
        let challenge = STANDARD.encode(synthetic_type_two_challenge());
        let endpoint = server.url("/EWS/Exchange.asmx");
        let anonymous_request = server
            .mock_async(|when, then| {
                when.method("POST")
                    .path("/EWS/Exchange.asmx")
                    .body_includes("RequestServerVersion")
                    .body_includes("SyncFolderItems")
                    .header_missing("Authorization");
                then.status(401).header("WWW-Authenticate", "NTLM");
            })
            .await;
        let negotiate_request = server
            .mock_async(|when, then| {
                when.method("POST")
                    .path("/EWS/Exchange.asmx")
                    .header_matches("Authorization", r"^NTLM TlRMTVNTUAAB");
                then.status(401)
                    .header("WWW-Authenticate", format!("NTLM {challenge}"));
            })
            .await;
        let authenticate_request = server
            .mock_async(|when, then| {
                when.method("POST")
                    .path("/EWS/Exchange.asmx")
                    .header_matches("Authorization", r"^NTLM TlRMTVNTUAAD");
                then.status(200)
                    .header("Content-Type", "text/xml; charset=utf-8")
                    .body(SYNC_FOLDER_ITEMS_PAGE);
            })
            .await;
        let certificate =
            Certificate::from_pem(httpmock::server::DEFAULT_CA_CERTIFICATE.as_bytes())
                .expect("synthetic HTTPS test CA parses");
        let client_builder = build_client_builder()
            .no_proxy()
            .add_root_certificate(certificate);
        let transport = EwsTransport::new_with_client_builder(
            &endpoint,
            "EXAMPLE\\ada".to_owned(),
            SecretString::from("synthetic-only"),
            client_builder,
        )
        .expect("EWS transport starts with the synthetic test CA");

        let responses = transport
            .execute(sync_folder_items_request(None))
            .await
            .expect("synthetic NTLMv2 exchange returns a parsed EWS page");

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses.first(), Some(ResponseClass::Success(_))));
        anonymous_request.assert_async().await;
        negotiate_request.assert_async().await;
        authenticate_request.assert_async().await;
    }

    #[tokio::test]
    async fn https_client_does_not_follow_redirects_with_an_authorization_header() {
        let origin = MockServer::start_async().await;
        let destination = MockServer::start_async().await;
        let destination_request = destination
            .mock_async(|when, then| {
                when.method("POST").path("/redirect-target");
                then.status(200);
            })
            .await;
        let redirect = origin
            .mock_async(|when, then| {
                when.method("POST").path("/EWS/Exchange.asmx");
                then.status(302)
                    .header("Location", destination.url("/redirect-target"));
            })
            .await;
        let certificate =
            Certificate::from_pem(httpmock::server::DEFAULT_CA_CERTIFICATE.as_bytes())
                .expect("synthetic HTTPS test CA parses");
        let client = build_client_builder()
            .no_proxy()
            .add_root_certificate(certificate)
            .build()
            .expect("synthetic HTTPS client builds");
        let mut authorization = reqwest::header::HeaderValue::from_static("NTLM synthetic-token");
        authorization.set_sensitive(true);

        let response = client
            .post(origin.url("/EWS/Exchange.asmx"))
            .header(AUTHORIZATION, authorization)
            .body("synthetic SOAP request")
            .send()
            .await
            .expect("the HTTPS redirect response is received");

        assert_eq!(response.status(), StatusCode::FOUND);
        redirect.assert_async().await;
        destination_request.assert_calls(0);
    }
}
