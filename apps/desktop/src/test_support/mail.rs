mod cache;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod providers;

#[cfg(feature = "ui-preview")]
pub(crate) use cache::FixtureCache;
#[cfg(test)]
pub(crate) use cache::fixture;
#[cfg(test)]
pub(crate) use fixtures::{
    SYNTHETIC_MESSAGE_LABEL_PREFIX, append_synthetic_messages, mail_profile_with_capabilities,
};
#[cfg(test)]
pub(crate) use providers::{
    MessageMoveActionResponse, OfflineProvider, PermissionProvider, ReadStateActionResponse,
    ReadStateProvider, message_move_provider,
};
