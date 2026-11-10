//! SQL schema and migrations for the encrypted mail and calendar cache.

use async_sqlite::Client;

use crate::store::{CacheFailure, CacheResult};

mod compatibility;
mod migrations;
#[cfg(test)]
mod tests;

use compatibility::apply_parallel_schema_compatibility;
use migrations::migrations;

pub(super) const MESSAGE_QUERY_VERSION: i64 = 2;

pub(super) async fn apply(client: &Client) -> CacheResult<()> {
    client
        .conn_mut_and_then(|connection| {
            apply_parallel_schema_compatibility(connection)?;
            migrations()
                .to_latest(connection)
                .map_err(|_| CacheFailure::Unavailable)
        })
        .await
        .map_err(|_| CacheFailure::Unavailable)?;
    Ok(())
}
