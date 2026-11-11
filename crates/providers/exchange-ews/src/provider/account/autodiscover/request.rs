use std::time::Duration;

use reqwest::{Client, Response, StatusCode, header::LOCATION, redirect::Policy};
use url::Url;

use super::{
    DiscoveryState, MAX_REDIRECTS,
    candidates::MAX_MAILBOX_ADDRESS_BYTES,
    pox::{ACCEPTABLE_RESPONSE_SCHEMA, REQUEST_NAMESPACE},
};
use crate::error::EwsError;

const MAX_REQUEST_BYTES: usize = 8 * 1024;
pub(super) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
pub(super) const AUTODISCOVER_PATH: &str = "/autodiscover/autodiscover.xml";

pub(super) fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

pub(super) fn build_client() -> Result<Client, EwsError> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .http1_only()
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| EwsError::ServiceUnavailable)
}

pub(super) async fn post_autodiscover(
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

pub(super) fn request_document(mailbox_address: &str) -> Result<String, EwsError> {
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

pub(super) async fn read_limited_body(mut response: Response) -> Result<Vec<u8>, EwsError> {
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

pub(super) fn validate_content_length(content_length: Option<u64>) -> Result<(), EwsError> {
    if content_length.is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(EwsError::ResourceLimitExceeded);
    }
    Ok(())
}

pub(super) fn append_response_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), EwsError> {
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
pub(super) fn parse_redirect_url(base: &Url, location: &str) -> Result<Url, EwsError> {
    let url = base
        .join(location)
        .map_err(|_| EwsError::AutodiscoverFailed)?;
    validate_discovery_url(url)
}

pub(super) fn validate_discovery_url(url: Url) -> Result<Url, EwsError> {
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

pub(super) fn enqueue_http_redirect(
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

pub(super) fn enqueue_redirect(state: &mut DiscoveryState, redirect: Url) -> Result<(), EwsError> {
    count_redirect(state)?;
    state.explicit_confirmation_required = true;
    state.candidates.push_front(redirect);
    Ok(())
}

pub(super) fn count_redirect(state: &mut DiscoveryState) -> Result<(), EwsError> {
    state.redirect_count = state
        .redirect_count
        .checked_add(1)
        .ok_or(EwsError::AutodiscoverFailed)?;
    if state.redirect_count > MAX_REDIRECTS {
        return Err(EwsError::AutodiscoverFailed);
    }
    Ok(())
}

pub(super) fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}
