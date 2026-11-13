use std::{
    env::{self, VarError},
    io,
    sync::{Arc, mpsc},
};

#[cfg(not(feature = "ui-preview"))]
use einfach_mail_account_router::AccountProviderRouter;
use einfach_mail_application::AccountProvider;
#[cfg(not(feature = "ui-preview"))]
use einfach_mail_application::AccountWorkflow;
#[cfg(not(feature = "ui-preview"))]
use einfach_mail_application::{AccountSessionFactory, AccountSessionWorkflowFactory};
#[cfg(not(feature = "ui-preview"))]
use einfach_mail_exchange_ews::ExchangeEwsProvider;
#[cfg(not(feature = "ui-preview"))]
use einfach_mail_microsoft_graph::{CallbackPort, MicrosoftGraphProvider};
#[cfg(not(feature = "ui-preview"))]
use einfach_mail_persistence::{SqliteAccountCacheFactory, XdgExchangeEwsAccountSettingsStore};
use gpui_kit::*;
#[cfg(not(feature = "ui-preview"))]
use std::ffi::OsStr;
#[cfg(feature = "ui-preview")]
use tempfile::TempDir;
#[cfg(not(feature = "ui-preview"))]
use uuid::Uuid;

use super::{NavigationSurfaceAppearance, Workspace};
use crate::features::{
    AccountServices, AppearanceSettingsStore, SetupIssue, bind_calendar_event_navigation_keys,
    bind_event_composer_keys, bind_mail_composer_keys, bind_message_navigation_keys,
};

#[cfg(not(feature = "ui-preview"))]
const CLIENT_ID_ENV: &str = "EINFACH_MAIL_ENTRA_CLIENT_ID";
#[cfg(not(feature = "ui-preview"))]
const CALLBACK_PORT_ENV: &str = "EINFACH_MAIL_ENTRA_CALLBACK_PORT";
#[cfg(not(feature = "ui-preview"))]
const EXPERIMENTAL_TRANSLUCENCY_ENV: &str = "EINFACH_MAIL_EXPERIMENTAL_TRANSLUCENCY";
#[cfg(not(feature = "ui-preview"))]
const WAYLAND_DISPLAY_ENV: &str = "WAYLAND_DISPLAY";
#[cfg(feature = "ui-preview")]
const PREVIEW_SCENARIO_ENV: &str = "EINFACH_MAIL_UI_PREVIEW_SCENARIO";

#[cfg(not(feature = "ui-preview"))]
fn navigation_surface_appearance_from_environment(
    experimental_opt_in: Option<&OsStr>,
    wayland_display: Option<&OsStr>,
) -> NavigationSurfaceAppearance {
    let opted_in = experimental_opt_in == Some(OsStr::new("1"));
    let wayland_display_is_available = wayland_display.is_some_and(|display| !display.is_empty());
    if opted_in && wayland_display_is_available {
        NavigationSurfaceAppearance::Translucent
    } else {
        NavigationSurfaceAppearance::Opaque
    }
}

#[cfg(not(feature = "ui-preview"))]
fn configured_navigation_surface_appearance() -> NavigationSurfaceAppearance {
    #[cfg(target_os = "linux")]
    {
        navigation_surface_appearance_from_environment(
            env::var_os(EXPERIMENTAL_TRANSLUCENCY_ENV).as_deref(),
            env::var_os(WAYLAND_DISPLAY_ENV).as_deref(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        NavigationSurfaceAppearance::Opaque
    }
}

fn window_options(appearance: NavigationSurfaceAppearance) -> WindowOptions {
    WindowOptions {
        window_background: appearance.window_background(),
        ..WindowOptions::default()
    }
}

#[cfg(not(feature = "ui-preview"))]
fn configured_account_services()
-> Result<AccountServices<AccountProviderRouter<XdgExchangeEwsAccountSettingsStore>>, SetupIssue> {
    let (graph_provider, graph_setup_issue) = configured_graph_provider();
    let settings_store = XdgExchangeEwsAccountSettingsStore::new()
        .map_err(|_| SetupIssue::ExchangeSettingsUnavailable)?;
    let ews_provider = ExchangeEwsProvider::new(settings_store);
    let provider =
        AccountProviderRouter::new(graph_provider, ews_provider, graph_setup_issue.is_none());
    let account_workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let account_session_factory: Arc<dyn AccountSessionFactory> = Arc::new(
        AccountSessionWorkflowFactory::new(provider, SqliteAccountCacheFactory),
    );
    let services = AccountServices::new(account_workflow, account_session_factory);
    Ok(match graph_setup_issue {
        Some(issue) => services.with_microsoft365_setup_issue(issue),
        None => services,
    })
}

#[cfg(not(feature = "ui-preview"))]
fn configured_graph_provider() -> (MicrosoftGraphProvider, Option<SetupIssue>) {
    let client_id = match env::var(CLIENT_ID_ENV) {
        Ok(client_id_value) => match Uuid::parse_str(&client_id_value) {
            Ok(client_id) => client_id,
            Err(_) => {
                return (
                    MicrosoftGraphProvider::new_unconfigured(),
                    Some(SetupIssue::InvalidClientId),
                );
            }
        },
        Err(VarError::NotPresent) => {
            return (
                MicrosoftGraphProvider::new_unconfigured(),
                Some(SetupIssue::MissingClientId),
            );
        }
        Err(VarError::NotUnicode(_)) => {
            return (
                MicrosoftGraphProvider::new_unconfigured(),
                Some(SetupIssue::InvalidClientId),
            );
        }
    };
    let callback_port_value = match env::var(CALLBACK_PORT_ENV) {
        Ok(port_value) => Some(port_value),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(_)) => {
            return (
                MicrosoftGraphProvider::new_unconfigured(),
                Some(SetupIssue::InvalidCallbackPort),
            );
        }
    };
    match configured_callback_port(callback_port_value.as_deref()) {
        Ok(callback_port) => (MicrosoftGraphProvider::new(client_id, callback_port), None),
        Err(issue) => (MicrosoftGraphProvider::new_unconfigured(), Some(issue)),
    }
}

#[cfg(not(feature = "ui-preview"))]
fn configured_callback_port(port_text: Option<&str>) -> Result<CallbackPort, SetupIssue> {
    let Some(port_text) = port_text else {
        return Err(SetupIssue::MissingCallbackPort);
    };
    let callback_port = CallbackPort::parse_environment_value(Some(port_text))
        .map_err(|_| SetupIssue::InvalidCallbackPort)?;
    match callback_port {
        CallbackPort::Ephemeral => Err(SetupIssue::InvalidCallbackPort),
        CallbackPort::Fixed(_) => Ok(callback_port),
    }
}

struct ApplicationSetup<P: AccountProvider + 'static> {
    services: Result<AccountServices<P>, SetupIssue>,
    appearance_settings_store: io::Result<AppearanceSettingsStore>,
    navigation_surface_appearance: NavigationSurfaceAppearance,
    #[cfg(feature = "ui-preview")]
    preview_settings_directory: Option<Arc<TempDir>>,
}

#[cfg(feature = "ui-preview")]
#[derive(Clone)]
struct PreviewSettingsDirectory {
    _directory: Arc<TempDir>,
}

#[cfg(feature = "ui-preview")]
impl Global for PreviewSettingsDirectory {}

#[cfg(not(feature = "ui-preview"))]
pub(crate) fn run_application() -> io::Result<()> {
    run_application_with(ApplicationSetup {
        services: configured_account_services(),
        appearance_settings_store: AppearanceSettingsStore::new(),
        navigation_surface_appearance: configured_navigation_surface_appearance(),
    })
}

#[cfg(feature = "ui-preview")]
pub(crate) fn run_preview_application() -> io::Result<()> {
    let scenario = preview_scenario_from_environment()?;
    let directory = Arc::new(tempfile::tempdir()?);
    let appearance_settings_store =
        AppearanceSettingsStore::at_path(directory.path().join("preferences.json"));

    run_application_with(ApplicationSetup {
        services: Ok(super::preview::account_services(scenario)),
        appearance_settings_store: Ok(appearance_settings_store),
        navigation_surface_appearance: NavigationSurfaceAppearance::Opaque,
        preview_settings_directory: Some(directory),
    })
}

#[cfg(feature = "ui-preview")]
fn preview_scenario_from_environment() -> io::Result<super::preview::PreviewScenario> {
    let value = match env::var(PREVIEW_SCENARIO_ENV) {
        Ok(value) => Some(value),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{PREVIEW_SCENARIO_ENV} must contain valid Unicode"),
            ));
        }
    };
    super::preview::PreviewScenario::parse(value.as_deref())
}

fn run_application_with<P: AccountProvider + 'static>(
    setup: ApplicationSetup<P>,
) -> io::Result<()> {
    let (window_open_attempt_sender, window_open_attempt_receiver) =
        mpsc::channel::<io::Result<()>>();
    application().with_assets(assets::Assets).run(move |cx| {
        initialize_application(cx);
        open_workspace_window(setup, window_open_attempt_sender, cx);
    });

    receive_window_open_attempt(window_open_attempt_receiver)
}

fn initialize_application(cx: &mut App) {
    init(cx);
    bind_message_navigation_keys(cx);
    bind_mail_composer_keys(cx);
    bind_event_composer_keys(cx);
    bind_calendar_event_navigation_keys(cx);
}

fn open_workspace_window<P: AccountProvider + 'static>(
    setup: ApplicationSetup<P>,
    window_open_attempt_sender: mpsc::Sender<io::Result<()>>,
    cx: &mut App,
) {
    #[cfg(feature = "ui-preview")]
    if let Some(directory) = setup.preview_settings_directory {
        cx.set_global(PreviewSettingsDirectory {
            _directory: directory,
        });
    }
    let account_services = setup.services;
    let appearance_settings_store = setup.appearance_settings_store;
    let navigation_surface_appearance = setup.navigation_surface_appearance;
    let window_open_attempt = open_window(
        window_options(navigation_surface_appearance),
        cx,
        |window, cx| {
            cx.new(|cx| {
                Workspace::new_for_window(account_services, appearance_settings_store, window, cx)
                    .with_navigation_surface_appearance(navigation_surface_appearance)
            })
        },
    )
    .map(|_| ())
    .map_err(|error| {
        io::Error::other(format!("failed to open the Einfach Mail window: {error:#}"))
    });
    if window_open_attempt.is_err() {
        cx.quit();
    }
    window_open_attempt_sender
        .send(window_open_attempt)
        .expect("the main thread keeps the receiver alive until GPUI exits");
}

fn receive_window_open_attempt(
    window_open_attempt_receiver: mpsc::Receiver<io::Result<()>>,
) -> io::Result<()> {
    match window_open_attempt_receiver.try_recv() {
        Ok(window_open_attempt) => window_open_attempt,
        Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => Err(io::Error::other(
            "GPUI exited before opening the Einfach Mail window",
        )),
    }
}

#[cfg(all(test, not(feature = "ui-preview")))]
mod tests;
