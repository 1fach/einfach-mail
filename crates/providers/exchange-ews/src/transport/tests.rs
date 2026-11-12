use super::{
    DigestAlgorithm, EwsCommand, EwsRequestError, EwsRequestMode, EwsTransport, EwsWriteError,
    MAX_SOAP_DEPTH, build_client_builder, digest_for_hash_oid, digest_for_signature_oid,
    map_status, parse_endpoint, parse_soap_response, serialize_soap_request, validate_soap_header,
};
use super::{tls::ConnectionIdentity, worker::EwsTransportState};
use crate::auth::synthetic_type_two_challenge;
use crate::error::EwsError;
use crate::provider::read_state::{
    confirm_update, current_item_version, get_item_request, parse_get_item_response,
    parse_update_item_response, serialize_update_item_request,
};
use crate::wire::ExchangeProductVersion;
use base64::{Engine, engine::general_purpose::STANDARD};
use einfach_mail_domain::{MessageId, ReadState};
use ews::{
    BaseFolderId, BaseShape, ItemShape, PathToElement, ResponseClass,
    sync_folder_items::{SyncFolderItems, SyncScope},
};
use httpmock::{Mock, MockServer};
use reqwest::{Certificate, StatusCode, header::AUTHORIZATION};
use secrecy::SecretString;
use tokio::sync::{mpsc, oneshot};
use url::Url;

const SYNC_FOLDER_ITEMS_PAGE: &[u8] = include_bytes!("..//fixtures/sync-folder-items-page.xml");
const GET_ITEM_VERSIONED: &[u8] = include_bytes!("..//fixtures/get-item-versioned.xml");
const UPDATE_ITEM_SUCCESS: &[u8] = include_bytes!("..//fixtures/update-item-success.xml");

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
        "https://exchange.example.test:0/EWS/Exchange.asmx",
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
    let wrong_namespace = br#"<s:Envelope xmlns:s="urn:invalid"><s:Header/><s:Body/></s:Envelope>"#;

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
    let response = parse_soap_response::<SyncFolderItems>(SYNC_FOLDER_ITEMS_PAGE)
        .expect("synthetic SOAP response parses");

    assert!(response.server_version.is_none());
    let mut responses = response.messages;
    assert_eq!(responses.len(), 1);
    let ResponseClass::Success(response) = responses.pop().expect("one response exists") else {
        panic!("synthetic page has a successful response class");
    };
    assert_eq!(response.sync_state, "synthetic-page-cursor");
    assert!(!response.includes_last_item_in_range);
    assert_eq!(response.changes.inner.len(), 1);
}

#[tokio::test]
async fn updates_read_state_after_a_fresh_authenticated_item_read() {
    let server = MockServer::start_async().await;
    let challenge = STANDARD.encode(synthetic_type_two_challenge());
    let endpoint = server.url("/EWS/Exchange.asmx");
    let anonymous_request = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/EWS/Exchange.asmx")
                .body_includes("GetItem")
                .header_missing("Authorization");
            then.status(401).header("WWW-Authenticate", "NTLM");
        })
        .await;
    let negotiate_request = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/EWS/Exchange.asmx")
                .body_includes("GetItem")
                .header_matches("Authorization", r"^NTLM TlRMTVNTUAAB");
            then.status(401)
                .header("WWW-Authenticate", format!("NTLM {challenge}"));
        })
        .await;
    let authenticate_request = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/EWS/Exchange.asmx")
                .body_includes("GetItem")
                .header_matches("Authorization", r"^NTLM TlRMTVNTUAAD");
            then.status(200)
                .header("Content-Type", "text/xml; charset=utf-8")
                .body(GET_ITEM_VERSIONED);
        })
        .await;
    let update_request = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/EWS/Exchange.asmx")
                .body_includes("UpdateItem")
                .body_includes("SuppressReadReceipts=\"true\"")
                .body_includes("ConflictResolution=\"NeverOverwrite\"")
                .body_includes("ChangeKey=\"synthetic-change-key\"")
                .header_missing("Authorization");
            then.status(200)
                .header("Content-Type", "text/xml; charset=utf-8")
                .body(UPDATE_ITEM_SUCCESS);
        })
        .await;
    let unauthorized_update_request = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/EWS/Exchange.asmx")
                .body_includes("UpdateItem")
                .body_includes("ChangeKey=\"synthetic-stale-change-key\"")
                .header_missing("Authorization");
            then.status(401).header("WWW-Authenticate", "NTLM");
        })
        .await;
    let certificate = Certificate::from_pem(httpmock::server::DEFAULT_CA_CERTIFICATE.as_bytes())
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

    let message_id = MessageId::try_from("synthetic-message-id".to_owned())
        .expect("synthetic message ID is valid");
    let sequence = transport.sequence().await;
    let (get_item_xml, _) = sequence
        .execute_xml_with_size(get_item_request(&message_id))
        .await
        .expect("synthetic NTLMv2 exchange returns a GetItem response");
    let parsed_get_item =
        parse_get_item_response(&get_item_xml).expect("synthetic GetItem response parses");
    let (server_version, current) = current_item_version(parsed_get_item, &message_id)
        .expect("synthetic GetItem confirms the requested message");
    let product_version = server_version
        .as_ref()
        .and_then(ExchangeProductVersion::from_server_version_info)
        .expect("synthetic server version is complete");
    assert!(product_version.supports_read_receipt_suppression());
    let request = serialize_update_item_request(&current, ReadState::Read)
        .expect("synthetic read-state UpdateItem serializes");
    let update_response = sequence
        .execute_update_item(&request)
        .await
        .expect("authenticated connection accepts one UpdateItem request");
    let confirmation = parse_update_item_response(&update_response)
        .expect("synthetic UpdateItem confirmation parses");
    assert_eq!(
        confirm_update(confirmation, &message_id, ReadState::Read),
        Ok(ReadState::Read)
    );
    let stale_request = String::from_utf8(request)
        .expect("synthetic UpdateItem request is UTF-8")
        .replace("synthetic-change-key", "synthetic-stale-change-key");
    assert!(matches!(
        sequence.execute_update_item(stale_request.as_bytes()).await,
        Err(EwsWriteError::NotDispatched(EwsError::ServiceUnavailable))
    ));
    anonymous_request.assert_async().await;
    negotiate_request.assert_async().await;
    authenticate_request.assert_async().await;
    update_request.assert_async().await;
    unauthorized_update_request.assert_async().await;
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
    let certificate = Certificate::from_pem(httpmock::server::DEFAULT_CA_CERTIFICATE.as_bytes())
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

#[tokio::test]
async fn dropping_an_idle_transport_does_not_send_a_request() {
    let server = MockServer::start_async().await;
    let unexpected_request = server
        .mock_async(|when, then| {
            when.method("POST").path("/EWS/Exchange.asmx");
            then.status(StatusCode::OK);
        })
        .await;
    let transport = EwsTransport::new_with_client_builder(
        &server.url("/EWS/Exchange.asmx"),
        "EXAMPLE\\ada".to_owned(),
        SecretString::from("synthetic-only"),
        synthetic_client_builder(),
    )
    .expect("idle EWS transport starts with the synthetic test CA");

    drop(transport);
    unexpected_request.assert_calls(0);
}

#[tokio::test]
async fn queued_requests_are_classified_before_shutdown_dispatch() {
    let (commands, command_receiver) = mpsc::channel(2);
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let (read_response_sender, read_response_receiver) = oneshot::channel();
    commands
        .send(EwsCommand {
            document: b"synthetic read request".to_vec(),
            mode: EwsRequestMode::Read,
            response: read_response_sender,
        })
        .await
        .expect("queued command is accepted");
    let (update_response_sender, update_response_receiver) = oneshot::channel();
    commands
        .send(EwsCommand {
            document: b"synthetic UpdateItem".to_vec(),
            mode: EwsRequestMode::Update,
            response: update_response_sender,
        })
        .await
        .expect("queued update is accepted");
    shutdown_sender
        .send(())
        .expect("worker shutdown is signaled");

    super::worker::run_worker(
        command_receiver,
        shutdown_receiver,
        synthetic_worker_state("https://exchange.example.test/EWS/Exchange.asmx", None),
    )
    .await;

    let read_response = read_response_receiver
        .await
        .expect("worker answers queued read request");
    assert!(matches!(
        read_response,
        Err(EwsRequestError::Read(EwsError::ServiceUnavailable))
    ));
    let update_response = update_response_receiver
        .await
        .expect("worker answers queued update");
    assert!(matches!(
        update_response,
        Err(EwsRequestError::NotDispatched(EwsError::ServiceUnavailable))
    ));
}

#[tokio::test]
async fn shutdown_interrupts_in_flight_read_without_waiting_for_http_timeout() {
    let error = shutdown_in_flight_request(EwsRequestMode::Read, None).await;

    assert!(matches!(
        error,
        EwsRequestError::Read(EwsError::ServiceUnavailable)
    ));
}

#[tokio::test]
async fn shutdown_marks_in_flight_update_outcome_unknown() {
    let error = shutdown_in_flight_request(
        EwsRequestMode::Update,
        Some(synthetic_connection_identity()),
    )
    .await;

    assert!(matches!(error, EwsRequestError::OutcomeUnknown));
}

#[tokio::test]
async fn dropping_a_transport_during_a_read_does_not_wait_for_the_response() {
    let server = MockServer::start_async().await;
    let pending_request = server
        .mock_async(|when, then| {
            when.method("POST").path("/EWS/Exchange.asmx");
            then.status(StatusCode::OK)
                .delay(std::time::Duration::from_secs(5));
        })
        .await;
    let transport = EwsTransport::new_with_client_builder(
        &server.url("/EWS/Exchange.asmx"),
        "EXAMPLE\\ada".to_owned(),
        SecretString::from("synthetic-only"),
        synthetic_client_builder(),
    )
    .expect("EWS transport starts with the synthetic test CA");
    let (response_sender, response_receiver) = oneshot::channel();
    transport
        .commands
        .send(EwsCommand {
            document: b"synthetic SOAP request".to_vec(),
            mode: EwsRequestMode::Read,
            response: response_sender,
        })
        .await
        .expect("worker accepts request");
    wait_for_mock_call(&pending_request).await;

    let start = std::time::Instant::now();
    drop(transport);

    assert!(
        start.elapsed() < std::time::Duration::from_secs(3),
        "transport drop waits for the delayed HTTP response"
    );
    let response = response_receiver
        .await
        .expect("worker returns the shutdown result");
    assert!(matches!(
        response,
        Err(EwsRequestError::Read(EwsError::ServiceUnavailable))
    ));
}

async fn shutdown_in_flight_request(
    mode: EwsRequestMode,
    authenticated_connection: Option<ConnectionIdentity>,
) -> EwsRequestError {
    let server = MockServer::start_async().await;
    let pending_request = server
        .mock_async(|when, then| {
            when.method("POST").path("/EWS/Exchange.asmx");
            then.status(StatusCode::OK)
                .delay(std::time::Duration::from_secs(5));
        })
        .await;
    let endpoint = server.url("/EWS/Exchange.asmx");
    let state = synthetic_worker_state(&endpoint, authenticated_connection);
    let (commands, command_receiver) = mpsc::channel(1);
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let (response_sender, response_receiver) = oneshot::channel();
    let worker = tokio::spawn(super::worker::run_worker(
        command_receiver,
        shutdown_receiver,
        state,
    ));
    commands
        .send(EwsCommand {
            document: b"synthetic SOAP request".to_vec(),
            mode,
            response: response_sender,
        })
        .await
        .expect("worker accepts request");
    wait_for_mock_call(&pending_request).await;
    shutdown_sender
        .send(())
        .expect("worker shutdown is signaled");

    tokio::time::timeout(std::time::Duration::from_secs(3), worker)
        .await
        .expect("worker cancels the pending request")
        .expect("worker exits without panicking");
    let response = response_receiver
        .await
        .expect("worker returns the shutdown result");
    match response {
        Err(error) => error,
        Ok(_) => panic!("pending HTTP response should be interrupted"),
    }
}

async fn wait_for_mock_call(mock: &Mock<'_>) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while mock.calls_async().await == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("HTTP mock receives the pending request");
}

fn synthetic_worker_state(
    endpoint: &str,
    authenticated_connection: Option<ConnectionIdentity>,
) -> EwsTransportState {
    let endpoint = Url::parse(endpoint).expect("synthetic HTTPS endpoint parses");
    let target_name = endpoint
        .host_str()
        .expect("synthetic HTTPS endpoint has a host")
        .to_owned();
    let client = synthetic_client_builder()
        .build()
        .expect("synthetic HTTPS client builds");

    EwsTransportState {
        endpoint,
        target_name,
        username: "EXAMPLE\\ada".to_owned(),
        password: SecretString::from("synthetic-only"),
        client,
        authenticated_connection,
    }
}

fn synthetic_client_builder() -> reqwest::ClientBuilder {
    let certificate = Certificate::from_pem(httpmock::server::DEFAULT_CA_CERTIFICATE.as_bytes())
        .expect("synthetic HTTPS test CA parses");
    build_client_builder()
        .no_proxy()
        .add_root_certificate(certificate)
}

fn synthetic_connection_identity() -> ConnectionIdentity {
    ConnectionIdentity {
        local: "127.0.0.1:1234"
            .parse()
            .expect("synthetic local socket address parses"),
        remote: "127.0.0.1:5678"
            .parse()
            .expect("synthetic remote socket address parses"),
        certificate_hash: [0; 32],
    }
}
