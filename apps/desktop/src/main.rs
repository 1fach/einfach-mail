//! Starts Einfach Mail as a native GPUI desktop application.

mod app;
mod components;
mod features;
mod platform;
#[cfg(test)]
mod test_support;

fn main() -> std::io::Result<()> {
    platform::run_with_user_lock(app::run_application)?
}
