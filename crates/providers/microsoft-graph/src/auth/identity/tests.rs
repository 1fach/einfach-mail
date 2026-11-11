use url::Url;

use super::{
    AuthorizationExpectation, BASE_AUTHORIZATION_SCOPES, CALENDAR_READ_WRITE_AUTHORIZATION_SCOPES,
    Endpoints, MAIL_READ_WRITE_AUTHORIZATION_SCOPES, SEND_MAIL_AUTHORIZATION_SCOPES,
    authorization_scopes, validate_authorization_request,
};
use einfach_mail_application::AccountPermissionRequest;

fn expected_authorization_request<'expected>(
    endpoint: &'expected Url,
    scopes: &'expected [&'expected str],
) -> AuthorizationExpectation<'expected> {
    AuthorizationExpectation {
        endpoint,
        client_id: uuid::Uuid::from_u128(1),
        redirect_uri: "http://127.0.0.1:1234/callback",
        scopes,
    }
}

#[test]
fn mail_send_is_added_only_to_explicit_same_account_permission_review() {
    assert!(!BASE_AUTHORIZATION_SCOPES.contains(&"Mail.Send"));
    assert!(SEND_MAIL_AUTHORIZATION_SCOPES.contains(&"Mail.Send"));
    assert!(
        BASE_AUTHORIZATION_SCOPES
            .iter()
            .all(|scope| SEND_MAIL_AUTHORIZATION_SCOPES.contains(scope))
    );
    assert!(!SEND_MAIL_AUTHORIZATION_SCOPES.contains(&"Mail.Send.Shared"));
}

#[test]
fn accepts_the_exact_mail_send_scope_set_only_for_send_permission_review() {
    let endpoint = Url::parse("https://login.microsoftonline.com/organizations/authorize")
        .expect("synthetic endpoint is valid");
    let authorization_url = Url::parse(concat!(
        "https://login.microsoftonline.com/organizations/authorize?",
        "client_id=00000000-0000-0000-0000-000000000001&response_type=code&",
        "redirect_uri=http%3A%2F%2F127.0.0.1%3A1234%2Fcallback&state=test-state&",
        "scope=Calendars.Read%20Mail.Read%20Mail.Send%20User.Read%20offline_access&",
        "code_challenge_method=S256&code_challenge=synthetic-challenge"
    ))
    .expect("synthetic authorization URL is valid");
    assert!(
        validate_authorization_request(
            &authorization_url,
            &expected_authorization_request(&endpoint, &SEND_MAIL_AUTHORIZATION_SCOPES),
        )
        .is_ok()
    );
    assert!(
        validate_authorization_request(
            &authorization_url,
            &expected_authorization_request(&endpoint, &BASE_AUTHORIZATION_SCOPES),
        )
        .is_err()
    );
}

#[test]
fn accepts_only_the_expected_oauth_authorization_request() {
    let endpoint = Url::parse("https://login.microsoftonline.com/organizations/authorize")
        .expect("synthetic endpoint is valid");
    let authorization_url = Url::parse(concat!(
        "https://login.microsoftonline.com/organizations/authorize?",
        "client_id=00000000-0000-0000-0000-000000000001&response_type=code&",
        "redirect_uri=http%3A%2F%2F127.0.0.1%3A1234%2Fcallback&state=test-state&",
        "scope=Calendars.Read%20Mail.Read%20User.Read%20offline_access&code_challenge_method=S256&",
        "code_challenge=synthetic-challenge"
    ))
    .expect("synthetic authorization URL is valid");

    assert!(
        validate_authorization_request(
            &authorization_url,
            &expected_authorization_request(&endpoint, &BASE_AUTHORIZATION_SCOPES),
        )
        .is_ok()
    );

    let mut duplicate_parameter_url = authorization_url.clone();
    duplicate_parameter_url
        .query_pairs_mut()
        .append_pair("response_type", "token");
    assert!(
        validate_authorization_request(
            &duplicate_parameter_url,
            &expected_authorization_request(&endpoint, &BASE_AUTHORIZATION_SCOPES),
        )
        .is_err()
    );
}

#[test]
fn rejects_an_authorization_request_with_oidc_or_unapproved_scopes() {
    let endpoints = Endpoints {
        authorization: Url::parse("https://login.microsoftonline.com/organizations/authorize")
            .expect("synthetic endpoint is valid"),
        token: Url::parse("https://login.microsoftonline.com/organizations/token")
            .expect("synthetic endpoint is valid"),
        graph_me: Url::parse("https://graph.microsoft.com/v1.0/me")
            .expect("synthetic endpoint is valid"),
    };
    let authorization_url = Url::parse(concat!(
            "https://login.microsoftonline.com/organizations/authorize?",
            "response_type=code&redirect_uri=http%3A%2F%2F127.0.0.1%3A1234%2Fcallback&",
            "state=test-state&scope=openid%20Calendars.Read%20Mail.Read%20User.Read%20offline_access&nonce=nonce&",
            "code_challenge_method=S256&code_challenge=synthetic-challenge"
        ))
        .expect("synthetic authorization URL is valid");

    assert!(
        validate_authorization_request(
            &authorization_url,
            &expected_authorization_request(&endpoints.authorization, &BASE_AUTHORIZATION_SCOPES,),
        )
        .is_err()
    );
}

#[test]
fn requests_mail_write_scope_only_for_the_explicit_permission_update() {
    assert_eq!(
        authorization_scopes(AccountPermissionRequest::Current),
        &BASE_AUTHORIZATION_SCOPES
    );
    let requested = authorization_scopes(AccountPermissionRequest::MailReadWrite);
    assert_eq!(requested, &MAIL_READ_WRITE_AUTHORIZATION_SCOPES);
    assert!(requested.contains(&"Mail.ReadWrite"));
    assert!(!authorization_scopes(AccountPermissionRequest::Current).contains(&"Mail.ReadWrite"));
    let requested = authorization_scopes(AccountPermissionRequest::CalendarReadWrite);
    assert_eq!(requested, &CALENDAR_READ_WRITE_AUTHORIZATION_SCOPES);
    assert!(requested.contains(&"Calendars.ReadWrite"));
    assert!(!requested.contains(&"Mail.ReadWrite"));
    assert!(!requested.contains(&"Mail.Send"));
    assert_eq!(
        authorization_scopes(AccountPermissionRequest::SendMail),
        &SEND_MAIL_AUTHORIZATION_SCOPES
    );
}
