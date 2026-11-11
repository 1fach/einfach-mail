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
