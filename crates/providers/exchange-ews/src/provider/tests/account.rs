#[test]
fn ews_account_profile_advertises_view_only_calendar_capabilities() {
    let settings = ExchangeEwsAccountSettings::new(
        "https://exchange.example.test/EWS/Exchange.asmx".to_owned(),
        "ada@example.test".to_owned(),
        "EXAMPLE\\ada".to_owned(),
    );

    let profile = profile_for(&settings, None).expect("synthetic EWS profile is valid");

    assert_eq!(
        profile.capabilities(),
        AccountCapabilities::new(
            MailCapabilities::read_only(),
            CalendarCapabilities::view_only(),
        )
    );
}

#[test]
fn ews_profile_advertises_only_read_state_after_receipt_safe_version_evidence() {
    let settings = ExchangeEwsAccountSettings::new(
        "https://exchange.example.test/EWS/Exchange.asmx".to_owned(),
        "ada@example.test".to_owned(),
        "EXAMPLE\\ada".to_owned(),
    );
    let info = ServerVersionInfo {
        major_version: Some("15".to_owned()),
        minor_version: Some("0".to_owned()),
        major_build_number: Some("847".to_owned()),
        minor_build_number: Some("32".to_owned()),
        version: Some("Exchange2013_SP1".to_owned()),
    };
    let version = ExchangeProductVersion::from_server_version_info(&info)
        .expect("synthetic Exchange product version parses");

    let profile = profile_for(&settings, Some(version))
        .expect("synthetic EWS profile with supported version is valid");
    let capabilities = profile.capabilities();

    assert!(capabilities.mail().supports(MailAction::SetReadState));
    assert!(!capabilities.mail().supports(MailAction::Send));
    assert!(!capabilities.mail().supports(MailAction::Move));
    assert!(!capabilities.can_review_mail_write_permission());
    assert_eq!(capabilities.calendar(), CalendarCapabilities::view_only());
}

#[test]
fn rejects_invalid_utf8_password_bytes_from_the_credential_store() {
    assert!(matches!(
        decode_password(vec![0xff]),
        Err(EwsError::CredentialStoreUnavailable)
    ));
}
