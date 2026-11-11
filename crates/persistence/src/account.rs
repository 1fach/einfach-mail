#[cfg(target_os = "linux")]
mod ews_settings;
#[cfg(target_os = "linux")]
pub use ews_settings::XdgExchangeEwsAccountSettingsStore;
