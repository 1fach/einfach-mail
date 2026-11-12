use std::time::Duration;

use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue},
    redirect::Policy,
};
use secrecy::SecretString;
use tokio::sync::{mpsc, oneshot};
use url::Url;

use super::{
    EwsCommand, EwsRequestError, EwsRequestMode, EwsRequestPhase, shutdown_error,
    soap::{ReceivedResponse, receive_response},
    tls::{ConnectionIdentity, map_status, ntlm_challenge, ntlm_header, tls_server_end_point},
};
use crate::{
    auth::{NtlmHandshake, decode_challenge},
    error::EwsError,
};

pub(super) struct EwsTransportState {
    pub(super) endpoint: Url,
    pub(super) target_name: String,
    pub(super) username: String,
    pub(super) password: SecretString,
    pub(super) client: Client,
    pub(super) authenticated_connection: Option<ConnectionIdentity>,
}

impl EwsTransportState {
    async fn post_soap(
        &mut self,
        document: &[u8],
        mode: EwsRequestMode,
    ) -> Result<Vec<u8>, EwsRequestError> {
        match mode {
            EwsRequestMode::Read => self
                .post_soap_read(document)
                .await
                .map_err(EwsRequestError::Read),
            EwsRequestMode::Update => self.post_soap_update(document).await,
        }
    }

    async fn post_soap_read(&mut self, document: &[u8]) -> Result<Vec<u8>, EwsError> {
        let first = self.send(document, None).await?;
        if first.status == StatusCode::OK {
            if self.authenticated_connection.as_ref() != Some(&first.identity) {
                self.authenticated_connection = None;
                return Err(EwsError::AccessDenied);
            }
            return Ok(first.body);
        }
        if first.status != StatusCode::UNAUTHORIZED {
            return Err(map_status(first.status));
        }

        let mut handshake = NtlmHandshake::new(&self.username, &self.password)?;
        let initial_token = handshake.initial_token(&self.target_name)?;
        let initial_header = ntlm_header(&initial_token)?;
        let type_two = self.send(document, Some(initial_header)).await?;
        if type_two.status != StatusCode::UNAUTHORIZED {
            return Err(map_status(type_two.status));
        }
        let challenge = ntlm_challenge(&type_two.headers)?;
        let challenge_token = decode_challenge(challenge)?;
        let channel_binding = tls_server_end_point(&type_two.peer_certificate)?;
        let response_token =
            handshake.response_token(&self.target_name, &challenge_token, &channel_binding)?;
        let response_header = ntlm_header(&response_token)?;
        let authenticated = self.send(document, Some(response_header)).await?;
        if authenticated.identity != type_two.identity {
            self.authenticated_connection = None;
            return Err(EwsError::InvalidResponse);
        }
        if authenticated.status != StatusCode::OK {
            self.authenticated_connection = None;
            return Err(map_status(authenticated.status));
        }

        self.authenticated_connection = Some(authenticated.identity);
        Ok(authenticated.body)
    }

    async fn post_soap_update(&mut self, document: &[u8]) -> Result<Vec<u8>, EwsRequestError> {
        let Some(authenticated_connection) = self.authenticated_connection.clone() else {
            return Err(EwsRequestError::NotDispatched(EwsError::ServiceUnavailable));
        };
        let response = self
            .send(document, None)
            .await
            .map_err(|_| EwsRequestError::OutcomeUnknown)?;
        if response.status == StatusCode::UNAUTHORIZED {
            self.authenticated_connection = None;
            return Err(EwsRequestError::NotDispatched(EwsError::ServiceUnavailable));
        }
        if response.status != StatusCode::OK || response.identity != authenticated_connection {
            self.authenticated_connection = None;
            return Err(EwsRequestError::OutcomeUnknown);
        }
        Ok(response.body)
    }

    async fn send(
        &self,
        document: &[u8],
        authorization: Option<HeaderValue>,
    ) -> Result<ReceivedResponse, EwsError> {
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .header(CONTENT_TYPE, "text/xml; charset=utf-8")
            .body(document.to_vec());
        if let Some(authorization) = authorization {
            request = request.header(AUTHORIZATION, authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|_| EwsError::ServiceUnavailable)?;
        receive_response(response).await
    }
}

pub(super) async fn run_worker(
    mut commands: mpsc::Receiver<EwsCommand>,
    mut shutdown: oneshot::Receiver<()>,
    mut state: EwsTransportState,
) {
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                fail_queued_commands(&mut commands);
                break;
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                let response = tokio::select! {
                    biased;
                    _ = &mut shutdown => {
                        let _ = command.response.send(Err(shutdown_error(
                            command.mode,
                            EwsRequestPhase::Executing,
                        )));
                        fail_queued_commands(&mut commands);
                        break;
                    }
                    response = state.post_soap(&command.document, command.mode) => response,
                };
                let _ = command.response.send(response);
            }
        }
    }
}

fn fail_queued_commands(commands: &mut mpsc::Receiver<EwsCommand>) {
    while let Ok(command) = commands.try_recv() {
        let response = Err(shutdown_error(command.mode, EwsRequestPhase::Queued));
        let _ = command.response.send(response);
    }
}

pub(super) fn build_client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(35))
        .pool_idle_timeout(Duration::from_secs(60))
        .pool_max_idle_per_host(1)
        .http1_only()
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .tls_info(true)
}
