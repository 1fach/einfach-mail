mod bootstrap;
mod workspace;

pub(super) use bootstrap::run_application;
pub(crate) use workspace::{NavigationSurfaceAppearance, Workspace};
