use std::{
    sync::{Mutex as SyncMutex, OnceLock, mpsc as sync_mpsc},
    thread::{self, JoinHandle},
};

use ews::{Operation, OperationResponse};
use secrecy::SecretString;
use tokio::{
    runtime::Builder,
    sync::{Mutex, MutexGuard, mpsc, oneshot},
};
use url::Url;

use self::worker::{EwsTransportState, build_client_builder, run_worker};
use crate::error::EwsError;

mod soap;
#[cfg(test)]
mod tests;
mod tls;
mod worker;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SOAP_DEPTH: usize = 128;
const MAX_NTLM_TOKEN_BYTES: usize = 16 * 1024;
const REQUEST_CHANNEL_CAPACITY: usize = 16;
pub(crate) const SOAP_NAMESPACE: &[u8] = b"http://schemas.xmlsoap.org/soap/envelope/";

pub(crate) use self::soap::{parse_soap_response, serialize_soap_request, validate_soap_header};
#[cfg(test)]
use self::tls::{DigestAlgorithm, digest_for_hash_oid, digest_for_signature_oid, map_status};

pub(crate) struct EwsTransport {
    commands: mpsc::Sender<EwsCommand>,
    request_sequence: Mutex<()>,
    reaper: sync_mpsc::Sender<JoinHandle<()>>,
    shutdown: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

struct WorkerReaper {
    sender: sync_mpsc::Sender<JoinHandle<()>>,
    _thread: JoinHandle<()>,
}

impl WorkerReaper {
    fn start() -> Result<Self, EwsError> {
        let (sender, receiver) = sync_mpsc::channel::<JoinHandle<()>>();
        let worker = thread::Builder::new()
            .name("einfach-mail-ews-reaper".to_owned())
            .spawn(move || {
                while let Ok(worker) = receiver.recv() {
                    let _ = worker.join();
                }
            })
            .map_err(|_| EwsError::ServiceUnavailable)?;
        Ok(Self {
            sender,
            _thread: worker,
        })
    }
}

struct EwsCommand {
    document: Vec<u8>,
    mode: EwsRequestMode,
    response: oneshot::Sender<Result<Vec<u8>, EwsRequestError>>,
}

#[derive(Clone, Copy)]
enum EwsRequestMode {
    Read,
    Write,
}

#[derive(Clone, Copy)]
enum EwsRequestPhase {
    Queued,
    Executing,
}

enum EwsRequestError {
    Read(EwsError),
    NotDispatched(EwsError),
    OutcomeUnknown,
}

#[derive(Debug)]
pub(crate) enum EwsWriteError {
    NotDispatched(EwsError),
    OutcomeUnknown,
}

pub(crate) struct EwsSoapResponse<M> {
    pub(crate) messages: Vec<ews::ResponseClass<M>>,
    pub(crate) server_version: Option<ews::server_version::ServerVersionInfo>,
}

pub(crate) struct EwsRequestSequence<'a> {
    transport: &'a EwsTransport,
    _guard: MutexGuard<'a, ()>,
}

impl EwsTransport {
    pub(crate) fn new(
        endpoint: &str,
        username: String,
        password: SecretString,
    ) -> Result<Self, EwsError> {
        Self::new_with_client_builder(endpoint, username, password, build_client_builder())
    }

    fn new_with_client_builder(
        endpoint: &str,
        username: String,
        password: SecretString,
        client_builder: reqwest::ClientBuilder,
    ) -> Result<Self, EwsError> {
        let endpoint = parse_endpoint(endpoint)?;
        let target_name = endpoint
            .host_str()
            .filter(|host| !host.is_empty())
            .ok_or(EwsError::InvalidConnectionDetails)?
            .to_owned();
        let reaper = worker_reaper_sender()?;

        let (commands, command_receiver) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let (ready_sender, ready_receiver) = sync_mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("einfach-mail-ews-transport".to_owned())
            .spawn(move || {
                let runtime = match Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = ready_sender.send(Err(EwsError::ServiceUnavailable));
                        return;
                    }
                };
                let client = match client_builder.build() {
                    Ok(client) => client,
                    Err(_) => {
                        let _ = ready_sender.send(Err(EwsError::ServiceUnavailable));
                        return;
                    }
                };
                let state = EwsTransportState {
                    endpoint,
                    target_name,
                    username,
                    password,
                    client,
                    authenticated_connection: None,
                };
                if ready_sender.send(Ok(())).is_err() {
                    return;
                }
                runtime.block_on(run_worker(command_receiver, shutdown_receiver, state));
            })
            .map_err(|_| EwsError::ServiceUnavailable)?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                commands,
                request_sequence: Mutex::new(()),
                reaper,
                shutdown: Some(shutdown),
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(EwsError::ServiceUnavailable)
            }
        }
    }

    pub(crate) async fn sequence(&self) -> EwsRequestSequence<'_> {
        let guard = self.request_sequence.lock().await;
        EwsRequestSequence {
            transport: self,
            _guard: guard,
        }
    }

    pub(crate) async fn execute<O>(
        &self,
        operation: O,
    ) -> Result<EwsSoapResponse<<O::Response as OperationResponse>::Message>, EwsError>
    where
        O: Operation,
    {
        self.sequence().await.execute(operation).await
    }

    pub(crate) async fn execute_with_size<O>(
        &self,
        operation: O,
    ) -> Result<
        (
            EwsSoapResponse<<O::Response as OperationResponse>::Message>,
            usize,
        ),
        EwsError,
    >
    where
        O: Operation,
    {
        self.sequence().await.execute_with_size(operation).await
    }

    pub(crate) async fn execute_xml_with_size<O>(
        &self,
        operation: O,
    ) -> Result<(Vec<u8>, usize), EwsError>
    where
        O: Operation,
    {
        self.sequence().await.execute_xml_with_size(operation).await
    }
}

impl EwsRequestSequence<'_> {
    pub(crate) async fn execute<O>(
        &self,
        operation: O,
    ) -> Result<EwsSoapResponse<<O::Response as OperationResponse>::Message>, EwsError>
    where
        O: Operation,
    {
        self.execute_with_size(operation)
            .await
            .map(|(response, _)| response)
    }

    pub(crate) async fn execute_with_size<O>(
        &self,
        operation: O,
    ) -> Result<
        (
            EwsSoapResponse<<O::Response as OperationResponse>::Message>,
            usize,
        ),
        EwsError,
    >
    where
        O: Operation,
    {
        let document = serialize_soap_request(operation)?;
        let response = self
            .request(&document, EwsRequestMode::Read)
            .await
            .map_err(read_request_error)?;
        let response_size = response.len();
        Ok((parse_soap_response::<O>(&response)?, response_size))
    }

    pub(crate) async fn execute_xml_with_size<O>(
        &self,
        operation: O,
    ) -> Result<(Vec<u8>, usize), EwsError>
    where
        O: Operation,
    {
        let document = serialize_soap_request(operation)?;
        let response = self.execute_xml_read(&document).await?;
        let response_size = response.len();
        Ok((response, response_size))
    }

    pub(crate) async fn execute_xml_read(&self, document: &[u8]) -> Result<Vec<u8>, EwsError> {
        self.request(document, EwsRequestMode::Read)
            .await
            .map_err(read_request_error)
    }

    pub(crate) async fn execute_xml_write(
        &self,
        document: &[u8],
    ) -> Result<Vec<u8>, EwsWriteError> {
        self.request(document, EwsRequestMode::Write)
            .await
            .map_err(write_request_error)
    }

    async fn request(
        &self,
        document: &[u8],
        mode: EwsRequestMode,
    ) -> Result<Vec<u8>, EwsRequestError> {
        let before_dispatch = match mode {
            EwsRequestMode::Read => EwsRequestError::Read(EwsError::ResourceLimitExceeded),
            EwsRequestMode::Write => {
                EwsRequestError::NotDispatched(EwsError::ResourceLimitExceeded)
            }
        };
        if document.is_empty() || document.len() > MAX_REQUEST_BYTES {
            return Err(before_dispatch);
        }
        let (response_sender, response_receiver) = oneshot::channel();
        self.transport
            .commands
            .send(EwsCommand {
                document: document.to_vec(),
                mode,
                response: response_sender,
            })
            .await
            .map_err(|_| match mode {
                EwsRequestMode::Read => EwsRequestError::Read(EwsError::ServiceUnavailable),
                EwsRequestMode::Write => {
                    EwsRequestError::NotDispatched(EwsError::ServiceUnavailable)
                }
            })?;
        response_receiver.await.map_err(|_| match mode {
            EwsRequestMode::Read => EwsRequestError::Read(EwsError::ServiceUnavailable),
            EwsRequestMode::Write => EwsRequestError::OutcomeUnknown,
        })?
    }
}

fn read_request_error(error: EwsRequestError) -> EwsError {
    match error {
        EwsRequestError::Read(error) | EwsRequestError::NotDispatched(error) => error,
        EwsRequestError::OutcomeUnknown => EwsError::ServiceUnavailable,
    }
}

fn write_request_error(error: EwsRequestError) -> EwsWriteError {
    match error {
        EwsRequestError::NotDispatched(error) => EwsWriteError::NotDispatched(error),
        EwsRequestError::OutcomeUnknown | EwsRequestError::Read(_) => EwsWriteError::OutcomeUnknown,
    }
}

impl Drop for EwsTransport {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take()
            && let Err(error) = self.reaper.send(worker)
        {
            let _ = error.0.join();
        }
    }
}

/// Returns the process-wide reaper sender, starting its thread on first use.
fn worker_reaper_sender() -> Result<sync_mpsc::Sender<JoinHandle<()>>, EwsError> {
    static WORKER_REAPER: OnceLock<SyncMutex<Option<WorkerReaper>>> = OnceLock::new();

    let mut reaper = WORKER_REAPER
        .get_or_init(|| SyncMutex::new(None))
        .lock()
        .map_err(|_| EwsError::ServiceUnavailable)?;
    if let Some(reaper) = reaper.as_ref() {
        return Ok(reaper.sender.clone());
    }

    let reaper_worker = WorkerReaper::start()?;
    let sender = reaper_worker.sender.clone();
    *reaper = Some(reaper_worker);
    Ok(sender)
}

fn shutdown_error(mode: EwsRequestMode, phase: EwsRequestPhase) -> EwsRequestError {
    match mode {
        EwsRequestMode::Read => EwsRequestError::Read(EwsError::ServiceUnavailable),
        EwsRequestMode::Write if matches!(phase, EwsRequestPhase::Executing) => {
            EwsRequestError::OutcomeUnknown
        }
        EwsRequestMode::Write => EwsRequestError::NotDispatched(EwsError::ServiceUnavailable),
    }
}

pub(crate) fn parse_endpoint(endpoint: &str) -> Result<Url, EwsError> {
    if endpoint.is_empty() || endpoint.len() > 2 * 1024 || endpoint.chars().any(char::is_control) {
        return Err(EwsError::InvalidConnectionDetails);
    }
    let parsed = Url::parse(endpoint).map_err(|_| EwsError::InvalidConnectionDetails)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port() == Some(0)
    {
        return Err(EwsError::InvalidConnectionDetails);
    }
    Ok(parsed)
}
