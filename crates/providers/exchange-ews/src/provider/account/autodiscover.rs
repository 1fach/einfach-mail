use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use einfach_mail_application::{
    ExchangeEwsDiscoveryConfirmation, ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult,
};
use reqwest::{Client, Response, StatusCode};
use tokio::time::timeout;
use url::Url;

#[cfg(test)]
use self::{
    candidates::{
        MAX_SRV_RECORDS, SrvCandidate, candidate_url, domain_candidates, order_srv_candidates,
    },
    pox::{ACCEPTABLE_RESPONSE_SCHEMA, MAX_XML_DEPTH, REQUEST_NAMESPACE, RESPONSE_NAMESPACE},
    request::{
        AUTODISCOVER_PATH, CONNECT_TIMEOUT, MAX_RESPONSE_BYTES, REQUEST_TIMEOUT,
        append_response_chunk, parse_redirect_url, request_document, validate_content_length,
    },
};
use self::{
    candidates::{discovery_candidates, normalize_domain, parse_mailbox},
    pox::{PoxResponse, parse_pox_response},
    request::{
        build_client, count_redirect, enqueue_http_redirect, enqueue_redirect, is_redirect_status,
        origin, post_autodiscover, read_limited_body,
    },
};
use crate::error::EwsError;

const MAX_REQUESTS: usize = 12;
const MAX_REDIRECTS: usize = 5;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

mod candidates;
mod pox;
mod request;
#[cfg(test)]
mod tests;

#[derive(Default)]
pub(crate) struct EwsAutodiscover {
    pending: Mutex<Option<PendingDiscovery>>,
    next_challenge: AtomicU64,
}

struct PendingDiscovery {
    id: u64,
    state: DiscoveryState,
}

struct DiscoveryState {
    mailbox_address: String,
    candidates: VecDeque<Url>,
    approved_origins: HashSet<String>,
    visited_urls: HashSet<String>,
    visited_mailboxes: HashSet<String>,
    request_count: usize,
    redirect_count: usize,
    explicit_confirmation_required: bool,
}

impl DiscoveryState {
    fn new(mailbox_address: String, candidates: VecDeque<Url>) -> Self {
        let approved_origins = candidates.iter().take(2).map(origin).collect();
        let visited_mailboxes = HashSet::from([mailbox_address.to_ascii_lowercase()]);
        Self {
            mailbox_address,
            candidates,
            approved_origins,
            visited_urls: HashSet::new(),
            visited_mailboxes,
            request_count: 0,
            redirect_count: 0,
            explicit_confirmation_required: false,
        }
    }

    fn needs_confirmation(&self, candidate: &Url) -> bool {
        self.explicit_confirmation_required || !self.approved_origins.contains(&origin(candidate))
    }
}

enum CandidateAction {
    Exhausted,
    ConfirmationRequired,
    Request(Url),
}

/// Returns a result when discovery pauses or completes; `None` means try another candidate.
type DiscoveryStep = Result<Option<ExchangeEwsDiscoveryResult>, EwsError>;

impl EwsAutodiscover {
    pub(crate) async fn discover(
        &self,
        request: ExchangeEwsDiscoveryRequest,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        match request {
            ExchangeEwsDiscoveryRequest::Start { mailbox_address } => {
                self.start_discovery(mailbox_address).await
            }
            ExchangeEwsDiscoveryRequest::Continue(confirmation) => {
                self.continue_discovery(confirmation).await
            }
            ExchangeEwsDiscoveryRequest::Cancel(confirmation) => {
                self.cancel_pending(confirmation.challenge())?;
                Ok(ExchangeEwsDiscoveryResult::Cancelled)
            }
            _ => Err(EwsError::AutodiscoverFailed),
        }
    }

    async fn start_discovery(
        &self,
        mailbox_address: String,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        self.clear_pending()?;
        let mailbox = parse_mailbox(&mailbox_address)?;
        let domain = normalize_domain(mailbox.domain())?;
        let mailbox_address = mailbox.to_string();
        let client = build_client()?;
        timeout(
            DISCOVERY_TIMEOUT,
            self.start_with_client(&client, mailbox_address, domain),
        )
        .await
        .map_err(|_| EwsError::AutodiscoverFailed)?
    }

    async fn start_with_client(
        &self,
        client: &Client,
        mailbox_address: String,
        domain: String,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        let candidates = discovery_candidates(&domain).await;
        let state = DiscoveryState::new(mailbox_address, candidates);
        self.try_candidates(client, state).await
    }

    async fn continue_discovery(
        &self,
        confirmation: ExchangeEwsDiscoveryConfirmation,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        let client = build_client()?;
        timeout(
            DISCOVERY_TIMEOUT,
            self.continue_with_client(&client, confirmation),
        )
        .await
        .map_err(|_| EwsError::AutodiscoverFailed)?
    }

    async fn continue_with_client(
        &self,
        client: &Client,
        confirmation: ExchangeEwsDiscoveryConfirmation,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        let mut pending = self.take_pending(confirmation.challenge())?;
        if let Some(url) = pending.state.candidates.front() {
            pending.state.approved_origins.insert(origin(url));
        }
        pending.state.explicit_confirmation_required = false;
        self.try_candidates(client, pending.state).await
    }

    async fn try_candidates(
        &self,
        client: &Client,
        mut state: DiscoveryState,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        loop {
            match next_candidate_action(&mut state)? {
                CandidateAction::Exhausted => return Err(EwsError::AutodiscoverFailed),
                CandidateAction::ConfirmationRequired => return self.require_confirmation(state),
                CandidateAction::Request(candidate) => {
                    if let Some(discovery_result) =
                        self.try_candidate(client, &mut state, candidate).await?
                    {
                        return Ok(discovery_result);
                    }
                }
            }
        }
    }

    async fn try_candidate(
        &self,
        client: &Client,
        state: &mut DiscoveryState,
        candidate: Url,
    ) -> DiscoveryStep {
        let response = match post_autodiscover(client, &candidate, &state.mailbox_address).await {
            Ok(response) => response,
            Err(EwsError::ServiceUnavailable) => return Ok(None),
            Err(error) => return Err(error),
        };
        if response.status() == StatusCode::OK {
            self.handle_pox_response(response, state).await
        } else if is_redirect_status(response.status()) {
            enqueue_http_redirect(state, &candidate, response)?;
            Ok(None)
        } else {
            Ok(None)
        }
    }

    async fn handle_pox_response(
        &self,
        response: Response,
        state: &mut DiscoveryState,
    ) -> DiscoveryStep {
        let document = match read_limited_body(response).await {
            Ok(document) => document,
            Err(EwsError::ServiceUnavailable) => return Ok(None),
            Err(error) => return Err(error),
        };
        self.apply_pox_response(state, parse_pox_response(&document)?)
            .await
    }

    async fn apply_pox_response(
        &self,
        state: &mut DiscoveryState,
        response: PoxResponse,
    ) -> DiscoveryStep {
        match response {
            PoxResponse::Endpoint(endpoint) => Ok(Some(ExchangeEwsDiscoveryResult::Endpoint(
                endpoint.to_string(),
            ))),
            PoxResponse::RedirectUrl(url) => enqueue_redirect(state, url).map(|()| None),
            PoxResponse::RedirectAddress(address) => {
                count_redirect(state)?;
                self.follow_redirect_address(state, address).await?;
                Ok(None)
            }
            PoxResponse::NoEndpoint => Ok(None),
        }
    }

    async fn follow_redirect_address(
        &self,
        state: &mut DiscoveryState,
        address: String,
    ) -> Result<(), EwsError> {
        let address = parse_mailbox(&address)?;
        let mailbox_address = address.to_string();
        if !state
            .visited_mailboxes
            .insert(mailbox_address.to_ascii_lowercase())
        {
            return Err(EwsError::AutodiscoverFailed);
        }
        let domain = normalize_domain(address.domain())?;
        state.mailbox_address = mailbox_address;
        state.candidates = discovery_candidates(&domain).await;
        state.approved_origins = state.candidates.iter().take(2).map(origin).collect();
        state.explicit_confirmation_required = true;
        Ok(())
    }

    fn require_confirmation(
        &self,
        state: DiscoveryState,
    ) -> Result<ExchangeEwsDiscoveryResult, EwsError> {
        let confirmation = self.new_confirmation(&state)?;
        self.store_pending(confirmation.challenge(), state)?;
        Ok(ExchangeEwsDiscoveryResult::ConfirmationRequired(
            confirmation,
        ))
    }

    fn new_confirmation(
        &self,
        state: &DiscoveryState,
    ) -> Result<ExchangeEwsDiscoveryConfirmation, EwsError> {
        let destination = state
            .candidates
            .front()
            .map(|url| url.as_str().to_owned())
            .ok_or(EwsError::AutodiscoverFailed)?;
        let challenge = self.next_challenge()?;
        Ok(ExchangeEwsDiscoveryConfirmation::new(
            challenge,
            state.mailbox_address.clone(),
            destination,
        ))
    }

    fn next_challenge(&self) -> Result<u64, EwsError> {
        let mut current = self.next_challenge.load(Ordering::Relaxed);
        loop {
            let next = current.checked_add(1).ok_or(EwsError::AutodiscoverFailed)?;
            match self.next_challenge.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(next),
                Err(observed) => current = observed,
            }
        }
    }

    fn store_pending(&self, id: u64, state: DiscoveryState) -> Result<(), EwsError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| EwsError::ServiceUnavailable)?;
        *pending = Some(PendingDiscovery { id, state });
        Ok(())
    }

    fn take_pending(&self, challenge: u64) -> Result<PendingDiscovery, EwsError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| EwsError::ServiceUnavailable)?;
        if pending
            .as_ref()
            .is_some_and(|pending_discovery| pending_discovery.id == challenge)
        {
            pending.take().ok_or(EwsError::AutodiscoverFailed)
        } else {
            Err(EwsError::AutodiscoverFailed)
        }
    }

    fn cancel_pending(&self, challenge: u64) -> Result<(), EwsError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| EwsError::ServiceUnavailable)?;
        if pending
            .as_ref()
            .is_some_and(|pending_discovery| pending_discovery.id == challenge)
        {
            *pending = None;
        }
        Ok(())
    }

    fn clear_pending(&self) -> Result<(), EwsError> {
        *self
            .pending
            .lock()
            .map_err(|_| EwsError::ServiceUnavailable)? = None;
        Ok(())
    }
}

fn next_candidate_action(state: &mut DiscoveryState) -> Result<CandidateAction, EwsError> {
    while let Some(candidate) = state.candidates.pop_front() {
        if state.visited_urls.contains(candidate.as_str()) {
            continue;
        }
        if state.request_count >= MAX_REQUESTS {
            return Err(EwsError::AutodiscoverFailed);
        }
        if state.needs_confirmation(&candidate) {
            state.candidates.push_front(candidate);
            return Ok(CandidateAction::ConfirmationRequired);
        }
        state.visited_urls.insert(candidate.to_string());
        state.request_count += 1;
        return Ok(CandidateAction::Request(candidate));
    }
    Ok(CandidateAction::Exhausted)
}
