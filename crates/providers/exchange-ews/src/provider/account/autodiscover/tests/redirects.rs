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
