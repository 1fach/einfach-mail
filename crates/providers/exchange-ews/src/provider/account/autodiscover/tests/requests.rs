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
