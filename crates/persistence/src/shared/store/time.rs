use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{CacheFailure, CacheResult};

pub(crate) fn system_time_parts(timestamp: SystemTime) -> CacheResult<(i64, i64)> {
    match timestamp.duration_since(UNIX_EPOCH) {
        Ok(duration) => positive_system_time_parts(duration),
        Err(error) => negative_system_time_parts(error.duration()),
    }
}

fn positive_system_time_parts(duration: Duration) -> CacheResult<(i64, i64)> {
    let seconds = i64::try_from(duration.as_secs()).map_err(|_| CacheFailure::InvalidData)?;
    Ok((seconds, i64::from(duration.subsec_nanos())))
}

fn negative_system_time_parts(duration: Duration) -> CacheResult<(i64, i64)> {
    let seconds = i64::try_from(duration.as_secs()).map_err(|_| CacheFailure::InvalidData)?;
    let nanoseconds = i64::from(duration.subsec_nanos());
    if nanoseconds == 0 {
        return Ok((-seconds, 0));
    }
    let seconds = seconds
        .checked_neg()
        .and_then(|seconds| seconds.checked_sub(1))
        .ok_or(CacheFailure::InvalidData)?;
    Ok((seconds, 1_000_000_000 - nanoseconds))
}

pub(crate) fn system_time_from_parts(seconds: i64, nanoseconds: i64) -> CacheResult<SystemTime> {
    if !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(CacheFailure::InvalidData);
    }
    let subsecond = Duration::from_nanos(nanoseconds as u64);
    let timestamp = if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64) + subsecond)
    } else {
        let distance = Duration::from_secs(seconds.unsigned_abs())
            .checked_sub(subsecond)
            .ok_or(CacheFailure::InvalidData)?;
        UNIX_EPOCH.checked_sub(distance)
    };
    timestamp.ok_or(CacheFailure::InvalidData)
}
