use std::time::Duration;

use reqwest::{Client, ClientBuilder, redirect::Policy};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn build_client() -> Result<Client, reqwest::Error> {
    client_builder().build()
}

#[cfg(test)]
pub(super) fn build_fixture_client() -> Result<Client, reqwest::Error> {
    client_builder().no_proxy().build()
}

fn client_builder() -> ClientBuilder {
    Client::builder()
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("Einfach Mail/", env!("CARGO_PKG_VERSION")))
        .retry(reqwest::retry::never())
}
