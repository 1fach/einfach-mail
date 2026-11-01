//! Isolated Tokio execution for the GPUI application boundary.

use std::{future::Future, thread};

use tokio::{runtime::Builder, sync::oneshot};
use tokio_util::sync::CancellationToken;

use einfach_mail_application::AccountOperationError;

pub(super) async fn run<T, F, Fut>(operation: F) -> Result<T, AccountOperationError>
where
    T: Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, AccountOperationError>> + Send + 'static,
{
    let cancellation = CancellationToken::new();
    let _cancel_when_dropped = CancelWhenDropped(cancellation.clone());
    let worker_cancellation = cancellation.clone();
    let (response_sender, response_receiver) = oneshot::channel();

    spawn_worker(operation, worker_cancellation, response_sender)?;

    response_receiver
        .await
        .map_err(|_| AccountOperationError::ServiceUnavailable)?
}

fn spawn_worker<T, F, Fut>(
    operation: F,
    cancellation: CancellationToken,
    response_sender: oneshot::Sender<Result<T, AccountOperationError>>,
) -> Result<(), AccountOperationError>
where
    T: Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, AccountOperationError>> + Send + 'static,
{
    thread::Builder::new()
        .name("einfach-mail-account".to_owned())
        .spawn(move || send_operation_result(operation, cancellation, response_sender))
        .map(|_| ())
        .map_err(|_| AccountOperationError::ServiceUnavailable)
}

fn send_operation_result<T, F, Fut>(
    operation: F,
    cancellation: CancellationToken,
    response_sender: oneshot::Sender<Result<T, AccountOperationError>>,
) where
    T: Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, AccountOperationError>> + Send + 'static,
{
    let operation_result = run_operation(operation, cancellation);
    let _ = response_sender.send(operation_result);
}

fn run_operation<T, F, Fut>(
    operation: F,
    cancellation: CancellationToken,
) -> Result<T, AccountOperationError>
where
    T: Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, AccountOperationError>> + Send + 'static,
{
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| AccountOperationError::ServiceUnavailable)?;

    runtime.block_on(async move {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(AccountOperationError::SignInCancelled),
            operation_result = operation(cancellation.clone()) => operation_result,
        }
    })
}

struct CancelWhenDropped(CancellationToken);

impl Drop for CancelWhenDropped {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
