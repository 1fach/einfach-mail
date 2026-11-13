//! Starts Einfach Mail as a native GPUI desktop application.

mod app;
mod components;
mod features;
#[cfg(not(feature = "ui-preview"))]
mod platform;
#[cfg(any(test, feature = "ui-preview"))]
mod test_support;
#[cfg(all(feature = "ui-preview", not(debug_assertions)))]
compile_error!("the synthetic UI preview is available only in debug builds");

#[cfg(not(feature = "ui-preview"))]
fn main() -> std::io::Result<()> {
    platform::run_with_user_lock(app::run_application)?
}

#[cfg(feature = "ui-preview")]
fn main() -> std::io::Result<()> {
    app::run_preview_application()
}
