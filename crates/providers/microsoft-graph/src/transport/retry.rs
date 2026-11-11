use std::time::{Duration, SystemTime};

use reqwest::{
    Response,
    header::{HeaderValue, RETRY_AFTER},
};
use tokio::sync::oneshot;

use crate::error::GraphError;

use super::GraphResponse;

const MAX_THROTTLE_RETRIES: u32 = 2;
const MAX_AUTOMATIC_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RetryAfter {
    Delay(Duration),
    Unrepresentable,
}

pub(super) enum RetryDecision {
    Retry(Duration),
    Stop(GraphError),
}

pub(super) fn retry_decision(response: &Response, attempt: u32) -> RetryDecision {
    let retry_after = retry_after_delay(
        response.headers().get(RETRY_AFTER),
        SystemTime::now(),
        attempt,
    );
    retry_decision_for(retry_after, attempt)
}

pub(super) fn rate_limit_error(response: &Response) -> GraphError {
    match retry_after_delay(response.headers().get(RETRY_AFTER), SystemTime::now(), 0) {
        RetryAfter::Delay(delay) => GraphError::RateLimited {
            retry_after: Some(delay),
        },
        RetryAfter::Unrepresentable => GraphError::RateLimited { retry_after: None },
    }
}

pub(super) fn retry_decision_for(retry_after: RetryAfter, attempt: u32) -> RetryDecision {
    match retry_after {
        RetryAfter::Delay(delay)
            if delay <= MAX_AUTOMATIC_RETRY_DELAY && attempt < MAX_THROTTLE_RETRIES =>
        {
            RetryDecision::Retry(delay)
        }
        RetryAfter::Delay(delay) => RetryDecision::Stop(GraphError::RateLimited {
            retry_after: Some(delay),
        }),
        RetryAfter::Unrepresentable => {
            RetryDecision::Stop(GraphError::RateLimited { retry_after: None })
        }
    }
}

pub(super) async fn wait_before_retry(
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
    delay: Duration,
) -> Option<()> {
    super::worker::while_interested(reply, tokio::time::sleep(delay)).await
}

fn retry_after_delay(
    retry_after_header: Option<&HeaderValue>,
    now: SystemTime,
    attempt: u32,
) -> RetryAfter {
    retry_after_header
        .and_then(|header| header.to_str().ok())
        .and_then(|retry_after_text| parse_retry_after(retry_after_text, now))
        .unwrap_or_else(|| RetryAfter::Delay(fallback_delay(attempt)))
}

pub(super) fn retry_after_hint(
    retry_after_header: Option<&HeaderValue>,
    now: SystemTime,
) -> Option<Duration> {
    let retry_after_text = retry_after_header?.to_str().ok()?;
    match parse_retry_after(retry_after_text, now)? {
        RetryAfter::Delay(delay) => Some(delay),
        RetryAfter::Unrepresentable => None,
    }
}

pub(super) fn parse_retry_after(retry_after_text: &str, now: SystemTime) -> Option<RetryAfter> {
    if !retry_after_text.is_empty() && retry_after_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(match retry_after_text.parse::<u64>() {
            Ok(seconds) => RetryAfter::Delay(Duration::from_secs(seconds)),
            Err(_) => RetryAfter::Unrepresentable,
        });
    }
    match httpdate::parse_http_date(retry_after_text) {
        Ok(retry_at) => Some(RetryAfter::Delay(
            retry_at.duration_since(now).unwrap_or(Duration::ZERO),
        )),
        Err(_) => None,
    }
}

pub(super) fn fallback_delay(attempt: u32) -> Duration {
    Duration::from_secs(2_u64.saturating_pow(attempt).min(60))
}
