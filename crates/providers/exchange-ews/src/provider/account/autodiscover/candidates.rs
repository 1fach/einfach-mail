use std::{collections::VecDeque, str::FromStr, time::Duration};

use addr_spec::AddrSpec;
use hickory_resolver::{Resolver, lookup::Lookup, proto::rr::RData};
use tokio::time::timeout;
use url::Url;

use super::request::{AUTODISCOVER_PATH, validate_discovery_url};
use crate::error::EwsError;

pub(super) const MAX_MAILBOX_ADDRESS_BYTES: usize = 2 * 1024;
pub(super) const MAX_SRV_RECORDS: usize = 8;
const DNS_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) struct SrvCandidate {
    pub(super) priority: u16,
    pub(super) weight: u16,
    pub(super) host: String,
    pub(super) url: Url,
}

pub(super) fn parse_mailbox(mailbox: &str) -> Result<AddrSpec, EwsError> {
    if mailbox.len() > MAX_MAILBOX_ADDRESS_BYTES {
        return Err(EwsError::InvalidConnectionDetails);
    }
    let address =
        AddrSpec::from_str(mailbox.trim()).map_err(|_| EwsError::InvalidConnectionDetails)?;
    if address.is_literal() {
        return Err(EwsError::InvalidConnectionDetails);
    }
    Ok(address)
}

pub(super) fn normalize_domain(domain: &str) -> Result<String, EwsError> {
    let url = Url::parse(&format!("https://{domain}/"))
        .map_err(|_| EwsError::InvalidConnectionDetails)?;
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or(EwsError::InvalidConnectionDetails)?;
    match url::Host::parse(host) {
        Ok(url::Host::Domain(domain)) if !domain.ends_with('.') => Ok(domain),
        _ => Err(EwsError::InvalidConnectionDetails),
    }
}

pub(super) async fn discovery_candidates(domain: &str) -> VecDeque<Url> {
    let mut candidates = domain_candidates(domain);
    candidates.extend(srv_candidates(domain).await);
    candidates
}

pub(super) fn domain_candidates(domain: &str) -> VecDeque<Url> {
    let mut candidates = VecDeque::new();
    for host in [format!("autodiscover.{domain}"), domain.to_owned()] {
        if let Ok(url) = candidate_url(&host, None) {
            candidates.push_back(url);
        }
    }
    candidates
}

async fn srv_candidates(domain: &str) -> Vec<Url> {
    let Some(records) = lookup_srv_records(domain).await else {
        return Vec::new();
    };
    let candidates = records
        .answers()
        .iter()
        .filter_map(srv_candidate)
        .collect::<Vec<_>>();
    order_srv_candidates(candidates)
}

pub(super) fn order_srv_candidates(mut candidates: Vec<SrvCandidate>) -> Vec<Url> {
    candidates.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| right.weight.cmp(&left.weight))
            .then_with(|| left.host.cmp(&right.host))
    });
    candidates
        .into_iter()
        .take(MAX_SRV_RECORDS)
        .map(|candidate| candidate.url)
        .collect()
}

async fn lookup_srv_records(domain: &str) -> Option<Lookup> {
    let resolver = Resolver::builder_tokio().ok()?.build().ok()?;
    let query = format!("_autodiscover._tcp.{domain}.");
    timeout(DNS_TIMEOUT, resolver.srv_lookup(query))
        .await
        .ok()?
        .ok()
}

fn srv_candidate(answer: &hickory_resolver::proto::rr::Record) -> Option<SrvCandidate> {
    let RData::SRV(record) = &answer.data else {
        return None;
    };
    let host = record.target.to_utf8();
    let host = host.trim_end_matches('.');
    if host.is_empty() || record.port == 0 {
        return None;
    }
    let url = candidate_url(host, Some(record.port)).ok()?;
    Some(SrvCandidate {
        priority: record.priority,
        weight: record.weight,
        host: host.to_owned(),
        url,
    })
}

pub(super) fn candidate_url(host: &str, port: Option<u16>) -> Result<Url, EwsError> {
    let mut url = Url::parse(&format!("https://{host}{AUTODISCOVER_PATH}"))
        .map_err(|_| EwsError::AutodiscoverFailed)?;
    if !matches!(url.host(), Some(url::Host::Domain(_))) {
        return Err(EwsError::AutodiscoverFailed);
    }
    if let Some(port) = port {
        url.set_port(Some(port))
            .map_err(|()| EwsError::AutodiscoverFailed)?;
    }
    validate_discovery_url(url)
}
