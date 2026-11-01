//! Isolated Tokio execution for the GPUI application boundary.

use std::{future::Future, thread};

use tokio::{runtime::Builder, sync::oneshot};
use tokio_util::sync::CancellationToken;

use einfach_mail_application::{AccountOperationCancellation, AccountOperationError};

pub(super) async fn run<T, E, F, Fut>(operation: F) -> Result<T, E>
where
    T: Send + 'static,
    E: From<AccountOperationError> + Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
{
    run_inner(operation, None).await
}

pub(super) async fn run_with_account_cancellation<T, F, Fut>(
    operation: F,
    account_cancellation: AccountOperationCancellation,
) -> Result<T, AccountOperationError>
where
    T: Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, AccountOperationError>> + Send + 'static,
{
    run_inner(operation, Some(account_cancellation)).await
}

async fn run_inner<T, F, Fut>(
    operation: F,
    account_cancellation: Option<AccountOperationCancellation>,
) -> Result<T, AccountOperationError>
where
    T: Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, AccountOperationError>> + Send + 'static,
{
    let cancellation = CancellationToken::new();
    let _cancel_when_dropped = CancelWhenDropped(cancellation.clone());
    let worker_cancellation = cancellation.clone();
    let (response_sender, response_receiver) = oneshot::channel();

    spawn_worker(
        operation,
        worker_cancellation,
        account_cancellation,
        response_sender,
    )?;

    response_receiver
        .await
        .map_err(|_| E::from(AccountOperationError::ServiceUnavailable))?
}

fn spawn_worker<T, E, F, Fut>(
    operation: F,
    cancellation: CancellationToken,
    account_cancellation: Option<AccountOperationCancellation>,
    response_sender: oneshot::Sender<Result<T, AccountOperationError>>,
) -> Result<(), AccountOperationError>
where
    T: Send + 'static,
    E: From<AccountOperationError> + Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
{
    thread::Builder::new()
        .name("einfach-mail-account".to_owned())
        .spawn(move || {
            send_operation_result(
                operation,
                cancellation,
                account_cancellation,
                response_sender,
            );
        })
        .map(|_| ())
        .map_err(|_| E::from(AccountOperationError::ServiceUnavailable))
}

fn send_operation_result<T, E, F, Fut>(
    operation: F,
    cancellation: CancellationToken,
    account_cancellation: Option<AccountOperationCancellation>,
    response_sender: oneshot::Sender<Result<T, AccountOperationError>>,
) where
    T: Send + 'static,
    E: From<AccountOperationError> + Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
{
    let operation_result = run_operation(operation, cancellation, account_cancellation);
    let _ = response_sender.send(operation_result);
}

fn run_operation<T, F, Fut>(
    operation: F,
    cancellation: CancellationToken,
    account_cancellation: Option<AccountOperationCancellation>,
) -> Result<T, AccountOperationError>
where
    T: Send + 'static,
    E: From<AccountOperationError> + Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
{
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| E::from(AccountOperationError::ServiceUnavailable))?;

    runtime.block_on(async move {
        match account_cancellation {
            Some(account_cancellation) => tokio::select! {
                biased;
                () = account_cancellation.cancelled() => {
                    cancellation.cancel();
                    Err(AccountOperationError::SignInCancelled)
                }
                () = cancellation.cancelled() => Err(AccountOperationError::SignInCancelled),
                operation_result = operation(cancellation.clone()) => operation_result,
            },
            None => tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(AccountOperationError::SignInCancelled),
                operation_result = operation(cancellation.clone()) => operation_result,
            },
        }
    })
}

struct CancelWhenDropped(CancellationToken);

impl Drop for CancelWhenDropped {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    use einfach_mail_application::{AccountOperationCancellation, AccountOperationError};

    use super::run_with_account_cancellation;

    #[tokio::test]
    async fn dropping_caller_after_commit_starts_does_not_interrupt_synchronous_work() {
        let cancellation = AccountOperationCancellation::new();
        let operation_cancellation = cancellation.clone();
        let (commit_started_sender, commit_started_receiver) = oneshot::channel();
        let (finish_commit_sender, finish_commit_receiver) = mpsc::channel();
        let (commit_finished_sender, commit_finished_receiver) = oneshot::channel();
        let task = tokio::spawn(run_with_account_cancellation(
            move |_worker_cancellation: CancellationToken| async move {
                operation_cancellation.begin_credential_commit()?;
                commit_started_sender
                    .send(())
                    .expect("test waits for credential commit start");
                finish_commit_receiver
                    .recv()
                    .expect("synthetic commit is released");
                commit_finished_sender
                    .send(())
                    .expect("test observes the completed synthetic commit");
                Ok::<(), AccountOperationError>(())
            },
            cancellation,
        ));

        commit_started_receiver
            .await
            .expect("worker reaches the credential commit");
        task.abort();
        assert!(task.await.is_err());
        finish_commit_sender
            .send(())
            .expect("worker waits for the commit release");
        tokio::time::timeout(Duration::from_secs(1), commit_finished_receiver)
            .await
            .expect("worker completes after the caller is dropped")
            .expect("worker reports the completed synthetic commit");
    }
}
