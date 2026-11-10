//! Account connection workflows and their provider port.

mod provider;
mod workflow;

pub use provider::{
    AccountConnectionRequest, AccountOperationError, AccountPermissionRequest, AccountProvider,
    ExchangeEwsConnectionInput, ExchangeEwsDiscoveryConfirmation, ExchangeEwsDiscoveryRequest,
    ExchangeEwsDiscoveryResult,
};
pub use workflow::{AccountOperationCancellation, AccountWorkflow};

#[cfg(test)]
#[path = "account/workflow/tests.rs"]
mod tests;
