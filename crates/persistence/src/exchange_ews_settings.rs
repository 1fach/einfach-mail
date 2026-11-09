//! Linux XDG storage for non-secret Exchange EWS connection settings.

use std::{
    fmt,
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use einfach_mail_application::{
    ExchangeEwsAccountSettings, ExchangeEwsAccountSettingsStore, ExchangeEwsSettingsError,
};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use xdg::BaseDirectories;

const SETTINGS_FILE: &str = "exchange-ews.json";
const SETTINGS_VERSION: u8 = 1;
const MAX_SETTINGS_BYTES: usize = 8 * 1024;
const MAX_ENDPOINT_BYTES: usize = 2 * 1024;
const MAX_MAILBOX_BYTES: usize = 512;
const MAX_USERNAME_BYTES: usize = 512;

/// Stores EWS endpoint and identity settings in the user's Linux XDG config directory.
#[derive(Clone)]
pub struct XdgExchangeEwsAccountSettingsStore {
    path: PathBuf,
}

impl XdgExchangeEwsAccountSettingsStore {
    /// Resolves the EWS settings path under the user's XDG config directory.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeEwsSettingsError`] when the platform cannot resolve the config path.
    pub fn new() -> Result<Self, ExchangeEwsSettingsError> {
        let path = BaseDirectories::with_prefix("einfach-mail")
            .get_config_file(SETTINGS_FILE)
            .ok_or(ExchangeEwsSettingsError)?;
        Ok(Self { path })
    }

    fn load_sync(&self) -> Result<Option<ExchangeEwsAccountSettings>, ExchangeEwsSettingsError> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ExchangeEwsSettingsError),
        };
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_SETTINGS_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ExchangeEwsSettingsError)?;
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(ExchangeEwsSettingsError);
        }

        let record = serde_json::from_slice::<StoredExchangeEwsSettings>(&bytes)
            .map_err(|_| ExchangeEwsSettingsError)?;
        if record.version != SETTINGS_VERSION
            || !valid_setting(&record.endpoint, MAX_ENDPOINT_BYTES)
            || !valid_setting(&record.mailbox_address, MAX_MAILBOX_BYTES)
            || !valid_setting(&record.username, MAX_USERNAME_BYTES)
        {
            return Err(ExchangeEwsSettingsError);
        }

        Ok(Some(ExchangeEwsAccountSettings::new(
            record.endpoint,
            record.mailbox_address,
            record.username,
        )))
    }

    fn save_sync(
        &self,
        settings: ExchangeEwsAccountSettings,
    ) -> Result<(), ExchangeEwsSettingsError> {
        if !valid_setting(settings.endpoint(), MAX_ENDPOINT_BYTES)
            || !valid_setting(settings.mailbox_address(), MAX_MAILBOX_BYTES)
            || !valid_setting(settings.username(), MAX_USERNAME_BYTES)
        {
            return Err(ExchangeEwsSettingsError);
        }

        let bytes = serde_json::to_vec(&StoredExchangeEwsSettingsRef {
            version: SETTINGS_VERSION,
            endpoint: settings.endpoint(),
            mailbox_address: settings.mailbox_address(),
            username: settings.username(),
        })
        .map_err(|_| ExchangeEwsSettingsError)?;
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(ExchangeEwsSettingsError);
        }

        let parent = self.path.parent().ok_or(ExchangeEwsSettingsError)?;
        create_private_directory(parent)?;
        let mut temporary = NamedTempFile::new_in(parent).map_err(|_| ExchangeEwsSettingsError)?;
        temporary
            .as_file()
            .set_permissions(Permissions::from_mode(0o600))
            .map_err(|_| ExchangeEwsSettingsError)?;
        temporary
            .write_all(&bytes)
            .map_err(|_| ExchangeEwsSettingsError)?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| ExchangeEwsSettingsError)?;
        temporary
            .persist(&self.path)
            .map_err(|_| ExchangeEwsSettingsError)?;
        sync_directory(parent)
    }

    fn delete_sync(&self) -> Result<(), ExchangeEwsSettingsError> {
        match fs::remove_file(&self.path) {
            Ok(()) => {
                let parent = self.path.parent().ok_or(ExchangeEwsSettingsError)?;
                sync_directory(parent)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ExchangeEwsSettingsError),
        }
    }
}

impl fmt::Debug for XdgExchangeEwsAccountSettingsStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XdgExchangeEwsAccountSettingsStore")
            .field("path", &"<redacted>")
            .finish()
    }
}

impl ExchangeEwsAccountSettingsStore for XdgExchangeEwsAccountSettingsStore {
    fn load(
        &self,
    ) -> impl std::future::Future<
        Output = Result<Option<ExchangeEwsAccountSettings>, ExchangeEwsSettingsError>,
    > + Send
    + '_ {
        let store = self.clone();
        async move { blocking::unblock(move || store.load_sync()).await }
    }

    fn save(
        &self,
        settings: ExchangeEwsAccountSettings,
    ) -> impl std::future::Future<Output = Result<(), ExchangeEwsSettingsError>> + Send + '_ {
        let store = self.clone();
        async move { blocking::unblock(move || store.save_sync(settings)).await }
    }

    fn delete(
        &self,
    ) -> impl std::future::Future<Output = Result<(), ExchangeEwsSettingsError>> + Send + '_ {
        let store = self.clone();
        async move { blocking::unblock(move || store.delete_sync()).await }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredExchangeEwsSettings {
    version: u8,
    endpoint: String,
    mailbox_address: String,
    username: String,
}

#[derive(Serialize)]
struct StoredExchangeEwsSettingsRef<'a> {
    version: u8,
    endpoint: &'a str,
    mailbox_address: &'a str,
    username: &'a str,
}

fn valid_setting(value: &str, max_bytes: usize) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn create_private_directory(path: &Path) -> Result<(), ExchangeEwsSettingsError> {
    fs::create_dir_all(path).map_err(|_| ExchangeEwsSettingsError)?;
    fs::set_permissions(path, Permissions::from_mode(0o700)).map_err(|_| ExchangeEwsSettingsError)
}

fn sync_directory(path: &Path) -> Result<(), ExchangeEwsSettingsError> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ExchangeEwsSettingsError)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt,
    };

    use einfach_mail_application::{
        ExchangeEwsAccountSettings, ExchangeEwsAccountSettingsStore, ExchangeEwsSettingsError,
    };
    use tempfile::tempdir;

    use super::{MAX_SETTINGS_BYTES, XdgExchangeEwsAccountSettingsStore};

    fn store(directory: &std::path::Path) -> XdgExchangeEwsAccountSettingsStore {
        XdgExchangeEwsAccountSettingsStore {
            path: directory.join("exchange-ews.json"),
        }
    }

    fn settings() -> ExchangeEwsAccountSettings {
        ExchangeEwsAccountSettings::new(
            "https://exchange.example.test/EWS/Exchange.asmx".to_owned(),
            "ada@example.test".to_owned(),
            "EXAMPLE\\ada".to_owned(),
        )
    }

    #[test]
    fn saves_private_non_secret_settings_and_replaces_them_atomically() {
        let directory = tempdir().expect("temporary directory is created");
        let store = store(directory.path());
        fs::set_permissions(directory.path(), Permissions::from_mode(0o755))
            .expect("temporary directory permissions are widened for the fixture");

        smol::block_on(store.save(settings())).expect("settings are saved");
        let bytes = fs::read(&store.path).expect("settings file is readable");
        let contents = String::from_utf8(bytes).expect("settings JSON is UTF-8");
        assert!(contents.contains("https://exchange.example.test/EWS/Exchange.asmx"));
        assert!(contents.contains("ada@example.test"));
        assert!(!contents.contains("password"));
        assert_eq!(
            fs::metadata(&store.path)
                .expect("settings metadata is readable")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(directory.path())
                .expect("settings directory metadata is readable")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let replacement = ExchangeEwsAccountSettings::new(
            "https://exchange2.example.test/EWS/Exchange.asmx".to_owned(),
            "ada@example.test".to_owned(),
            "EXAMPLE\\ada".to_owned(),
        );
        smol::block_on(store.save(replacement.clone())).expect("settings are replaced");
        assert_eq!(
            smol::block_on(store.load()).expect("settings are loaded"),
            Some(replacement)
        );
    }

    #[test]
    fn rejects_unknown_or_oversized_settings_and_deletes_idempotently() {
        let directory = tempdir().expect("temporary directory is created");
        let store = store(directory.path());
        fs::create_dir_all(directory.path()).expect("temporary directory exists");
        fs::write(
            &store.path,
            br#"{"version":2,"endpoint":"x","mailbox_address":"y","username":"z"}"#,
        )
        .expect("invalid settings fixture is written");

        assert_eq!(smol::block_on(store.load()), Err(ExchangeEwsSettingsError));
        let mut valid_settings = br#"{"version":1,"endpoint":"https://exchange.example.test/EWS/Exchange.asmx","mailbox_address":"ada@example.test","username":"EXAMPLE\\ada"}"#.to_vec();
        valid_settings.resize(MAX_SETTINGS_BYTES + 1, b' ');
        fs::write(&store.path, valid_settings).expect("oversized settings fixture is written");
        assert_eq!(smol::block_on(store.load()), Err(ExchangeEwsSettingsError));
        smol::block_on(store.delete()).expect("settings are removed");
        smol::block_on(store.delete()).expect("removing absent settings succeeds");
        assert_eq!(
            smol::block_on(store.load()).expect("settings are absent"),
            None
        );
    }
}
