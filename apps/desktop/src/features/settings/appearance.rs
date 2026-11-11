use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::PathBuf,
};

use gpui_kit::{WindowAppearance, component::ThemeMode};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use xdg::BaseDirectories;

const SETTINGS_FILE: &str = "preferences.json";
const MAX_SETTINGS_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AppearancePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl AppearancePreference {
    pub(crate) fn theme_mode(self, system_appearance: WindowAppearance) -> ThemeMode {
        match self {
            Self::System => system_appearance.into(),
            Self::Light => ThemeMode::Light,
            Self::Dark => ThemeMode::Dark,
        }
    }

    pub(crate) fn radio_index(self) -> usize {
        match self {
            Self::System => 0,
            Self::Light => 1,
            Self::Dark => 2,
        }
    }

    pub(crate) fn from_radio_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::System),
            1 => Some(Self::Light),
            2 => Some(Self::Dark),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AppearanceSettingsNotice {
    LoadFailed,
    SaveFailed,
}

impl AppearanceSettingsNotice {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::LoadFailed => {
                "The saved appearance setting could not be read. System appearance is active."
            }
            Self::SaveFailed => {
                "Appearance changed for this session, but Einfach Mail could not save it."
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppearanceSettingsStore {
    path: PathBuf,
}

impl AppearanceSettingsStore {
    pub(crate) fn new() -> io::Result<Self> {
        let path = BaseDirectories::with_prefix("einfach-mail")
            .get_config_file(SETTINGS_FILE)
            .ok_or_else(|| io::Error::other("could not resolve the XDG config file path"))?;
        Ok(Self { path })
    }

    pub(crate) fn load(&self) -> io::Result<AppearancePreference> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(AppearancePreference::System);
            }
            Err(error) => return Err(error),
        };

        let mut bytes = Vec::new();
        Read::take(file, (MAX_SETTINGS_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(invalid_settings_file());
        }

        let settings = serde_json::from_slice::<StoredAppearanceSettings>(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(settings.appearance)
    }

    pub(crate) fn save(&self, appearance: AppearancePreference) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&StoredAppearanceSettings { appearance })
            .map_err(io::Error::other)?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("the XDG config file has no parent directory"))?;

        fs::create_dir_all(parent)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        sync_directory(parent)
    }

    #[cfg(test)]
    pub(crate) fn at_path(path: PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Deserialize, Serialize)]
struct StoredAppearanceSettings {
    appearance: AppearancePreference,
}

fn invalid_settings_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "appearance settings file exceeds the supported size",
    )
}

fn sync_directory(path: &std::path::Path) -> io::Result<()> {
    OpenOptions::new().read(true).open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gpui_kit::{WindowAppearance, component::ThemeMode};
    use tempfile::tempdir;

    use super::{AppearancePreference, AppearanceSettingsStore, MAX_SETTINGS_BYTES};

    #[test]
    fn missing_settings_default_to_system_without_creating_a_file() {
        let directory = tempdir().expect("temporary directory is created");
        let path = directory.path().join("preferences.json");
        let store = AppearanceSettingsStore::at_path(path.clone());

        assert_eq!(
            store.load().expect("missing settings use the default"),
            AppearancePreference::System
        );
        assert!(!path.exists());
    }

    #[test]
    fn saves_and_loads_each_appearance_preference() {
        let directory = tempdir().expect("temporary directory is created");
        let store = AppearanceSettingsStore::at_path(directory.path().join("preferences.json"));

        for appearance in [
            AppearancePreference::System,
            AppearancePreference::Light,
            AppearancePreference::Dark,
        ] {
            store.save(appearance).expect("appearance is saved");
            assert_eq!(
                store.load().expect("saved appearance is loaded"),
                appearance
            );
        }
    }

    #[test]
    fn invalid_settings_are_reported_without_replacing_the_file() {
        let directory = tempdir().expect("temporary directory is created");
        let path = directory.path().join("preferences.json");
        fs::write(&path, b"not json").expect("invalid config fixture is written");
        let store = AppearanceSettingsStore::at_path(path.clone());

        let error = store.load().expect_err("invalid settings are rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(path).expect("invalid config remains available for recovery"),
            b"not json"
        );
    }

    #[test]
    fn settings_limit_is_enforced() {
        let directory = tempdir().expect("temporary directory is created");
        let path = directory.path().join("preferences.json");
        let store = AppearanceSettingsStore::at_path(path.clone());
        // Accept the exact limit and reject the first larger size to catch off-by-one errors.
        let mut exact_limit = br#"{"appearance":"dark"}"#.to_vec();
        exact_limit.resize(MAX_SETTINGS_BYTES, b' ');
        fs::write(&path, exact_limit).expect("maximum-size settings fixture is written");

        assert_eq!(
            store.load().expect("the exact size limit is accepted"),
            AppearancePreference::Dark
        );

        let mut over_limit = fs::read(&path).expect("settings fixture is readable");
        over_limit.push(b' ');
        fs::write(&path, over_limit).expect("oversized settings fixture is written");

        assert_eq!(
            store
                .load()
                .expect_err("one byte beyond the size limit is rejected")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn system_uses_window_appearance_and_fixed_modes_override_it() {
        assert_eq!(
            AppearancePreference::System.theme_mode(WindowAppearance::Dark),
            ThemeMode::Dark
        );
        assert_eq!(
            AppearancePreference::System.theme_mode(WindowAppearance::Light),
            ThemeMode::Light
        );
        assert_eq!(
            AppearancePreference::Light.theme_mode(WindowAppearance::Dark),
            ThemeMode::Light
        );
        assert_eq!(
            AppearancePreference::Dark.theme_mode(WindowAppearance::Light),
            ThemeMode::Dark
        );
    }
}
