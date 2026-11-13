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
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
#[cfg(not(feature = "ui-preview"))]
use std::ffi::OsStr;
#[cfg(feature = "ui-preview")]
use tempfile::TempDir;
#[cfg(not(feature = "ui-preview"))]
use uuid::Uuid;

use super::{NavigationSurfaceAppearance, Workspace};
use crate::components::{AppAssets, NAVIGATION_PANE_WIDTH, install_theme};
use crate::features::{
    AccountServices, AppearanceSettingsStore, MAIL_MINIMUM_WIDTH, SetupIssue,
    bind_calendar_event_navigation_keys, bind_event_composer_keys, bind_mail_composer_keys,
    bind_message_navigation_keys,
};

const WINDOW_MINIMUM_HEIGHT: Rems = rems(30.);
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

/// The smallest window that keeps the navigation pane at full width beside Mail's narrowest
/// layout.
fn minimum_window_size(rem_size: Pixels) -> Size<Pixels> {
    size(
        NAVIGATION_PANE_WIDTH.to_pixels(rem_size) + MAIL_MINIMUM_WIDTH.to_pixels(rem_size),
        WINDOW_MINIMUM_HEIGHT.to_pixels(rem_size),
    )
}

fn window_options(appearance: NavigationSurfaceAppearance, rem_size: Pixels) -> WindowOptions {
    WindowOptions {
        window_background: appearance.window_background(),
        window_min_size: Some(minimum_window_size(rem_size)),
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
    #[cfg(feature = "ui-preview")]
    preview_start: super::preview::PreviewStart,
    #[cfg(feature = "ui-preview")]
    preview_scenario: super::preview::PreviewScenario,
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
    let preview_start = preview_start_from_environment()?;
    let directory = Arc::new(tempfile::tempdir()?);
    let appearance_settings_store =
        AppearanceSettingsStore::at_path(directory.path().join("preferences.json"));
    if let Some(appearance) = preview_start.appearance {
        appearance_settings_store.save(appearance)?;
    }

    run_application_with(ApplicationSetup {
        services: Ok(super::preview::account_services(scenario)),
        appearance_settings_store: Ok(appearance_settings_store),
        navigation_surface_appearance: NavigationSurfaceAppearance::Opaque,
        preview_settings_directory: Some(directory),
        preview_start,
        preview_scenario: scenario,
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

#[cfg(feature = "ui-preview")]
fn preview_start_from_environment() -> io::Result<super::preview::PreviewStart> {
    use super::preview::{APPEARANCE_ENV, DESTINATION_ENV, PreviewStart, WINDOW_SIZE_ENV};

    let destination = preview_environment_value(DESTINATION_ENV)?;
    let appearance = preview_environment_value(APPEARANCE_ENV)?;
    let window_size = preview_environment_value(WINDOW_SIZE_ENV)?;
    PreviewStart::parse(
        destination.as_deref(),
        appearance.as_deref(),
        window_size.as_deref(),
    )
}

#[cfg(feature = "ui-preview")]
fn preview_environment_value(name: &str) -> io::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must contain valid Unicode"),
        )),
    }
}

fn run_application_with<P: AccountProvider + 'static>(
    setup: ApplicationSetup<P>,
) -> io::Result<()> {
    let (window_open_attempt_sender, window_open_attempt_receiver) =
        mpsc::channel::<io::Result<()>>();
    application().with_assets(AppAssets).run(move |cx| {
        initialize_application(cx);
        open_workspace_window(setup, window_open_attempt_sender, cx);
    });

    receive_window_open_attempt(window_open_attempt_receiver)
}

fn initialize_application(cx: &mut App) {
    init(cx);
    install_theme(cx);
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
    #[cfg(feature = "ui-preview")]
    let preview_start = setup.preview_start;
    #[cfg(feature = "ui-preview")]
    let preview_scenario = setup.preview_scenario;
    #[cfg_attr(not(feature = "ui-preview"), allow(unused_mut))]
    let mut options = window_options(navigation_surface_appearance, cx.theme().font_size);
    #[cfg(feature = "ui-preview")]
    if let Some(window_size) = preview_start.window_size {
        options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(window_size.width as f32), px(window_size.height as f32)),
            cx,
        )));
    }
    let window_open_attempt = open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            #[cfg_attr(not(feature = "ui-preview"), allow(unused_mut))]
            let mut workspace =
                Workspace::new_for_window(account_services, appearance_settings_store, window, cx)
                    .with_navigation_surface_appearance(navigation_surface_appearance);
            #[cfg(feature = "ui-preview")]
            {
                workspace.start_at_preview_destination(preview_start.destination);
                if preview_scenario.opens_exchange_form() {
                    workspace.open_exchange_form_at_preview_start(window, cx);
                }
            }
            workspace
        })
    })
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
