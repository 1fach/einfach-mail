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

const ENDPOINT_RESPONSE: &[u8] =
    include_bytes!("../../..//fixtures/autodiscover-endpoint-response.xml");
const EXCH_RESPONSE: &[u8] = include_bytes!("../../..//fixtures/autodiscover-exch-response.xml");
const REDIRECT_URL_RESPONSE: &[u8] =
    include_bytes!("../../..//fixtures/autodiscover-redirect-url-response.xml");
const REDIRECT_ADDRESS_RESPONSE: &[u8] =
    include_bytes!("../../..//fixtures/autodiscover-redirect-address-response.xml");

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
    let certificate = Certificate::from_pem(httpmock::server::DEFAULT_CA_CERTIFICATE.as_bytes())
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

include!("tests/candidates.rs");
include!("tests/pox.rs");
include!("tests/requests.rs");
include!("tests/redirects.rs");
