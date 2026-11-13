mod bootstrap;
#[cfg(feature = "ui-preview")]
mod preview;
mod workspace;

#[cfg(not(feature = "ui-preview"))]
pub(super) use bootstrap::run_application;
#[cfg(feature = "ui-preview")]
pub(super) use bootstrap::run_preview_application;
pub(crate) use workspace::{NavigationSurfaceAppearance, Workspace};
