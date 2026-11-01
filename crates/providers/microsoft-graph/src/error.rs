use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GraphError {
    WorkerStart,
    WorkerClosed,
    Transport,
    InvalidToken,
    AccessTokenRejected,
    AccessDenied,
    RateLimited { retry_after: Option<Duration> },
    InvalidResponse,
    UnsafeLink,
    InvalidRecord,
    ResponseTooLarge,
    PaginationLimitExceeded,
}
