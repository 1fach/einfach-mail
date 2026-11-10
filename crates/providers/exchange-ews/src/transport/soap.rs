use super::tls::ConnectionIdentity;
use super::{EwsSoapResponse, MAX_RESPONSE_BYTES, MAX_SOAP_DEPTH, SOAP_NAMESPACE};
use crate::error::EwsError;
use ews::{Operation, OperationResponse, server_version::ExchangeServerVersion, soap::Envelope};
use hyper_util::client::legacy::connect::HttpInfo;
use reqwest::{Response, StatusCode, tls::TlsInfo};
use sha2::{Digest, Sha256};

pub(crate) fn serialize_soap_request<O: Operation>(operation: O) -> Result<Vec<u8>, EwsError> {
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
) -> Result<EwsSoapResponse<<O::Response as OperationResponse>::Message>, EwsError> {
    validate_soap_header(document)?;
    let envelope = Envelope::<O::Response>::from_xml_document(document)
        .map_err(|_| EwsError::InvalidResponse)?;
    let mut server_version = None;
    for header in envelope.headers {
        if let ews::soap::Header::ServerVersionInfo(version) = header
            && server_version.replace(version).is_some()
        {
            return Err(EwsError::InvalidResponse);
        }
    }
    Ok(EwsSoapResponse {
        messages: envelope.body.into_response_messages(),
        server_version,
    })
}

pub(super) struct ReceivedResponse {
    pub(super) status: StatusCode,
    pub(super) headers: reqwest::header::HeaderMap,
    pub(super) identity: ConnectionIdentity,
    pub(super) peer_certificate: Vec<u8>,
    pub(super) body: Vec<u8>,
}

pub(super) async fn receive_response(mut response: Response) -> Result<ReceivedResponse, EwsError> {
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

pub(crate) fn validate_soap_header(document: &[u8]) -> Result<(), EwsError> {
    use quick_xml::{NsReader, events::Event, name::ResolveResult};

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
