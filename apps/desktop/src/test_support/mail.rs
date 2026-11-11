mod cache;
mod fixtures;
mod providers;

pub(crate) use cache::fixture;
pub(crate) use fixtures::{
    SYNTHETIC_MESSAGE_LABEL_PREFIX, append_synthetic_messages, mail_profile_with_capabilities,
};
pub(crate) use providers::{
    MessageMoveActionResponse, OfflineProvider, PermissionProvider, ReadStateActionResponse,
    ReadStateProvider, message_move_provider,
};
