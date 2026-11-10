use std::{
    env::{self, VarError},
    sync::{Arc, mpsc},
};

use einfach_mail_account_router::AccountProviderRouter;
use einfach_mail_application::{
    AccountSessionFactory, AccountSessionWorkflowFactory, AccountWorkflow,
};
use einfach_mail_exchange_ews::ExchangeEwsProvider;
use einfach_mail_microsoft_graph::{CallbackPort, MicrosoftGraphProvider};
use einfach_mail_persistence::{SqliteAccountCacheFactory, XdgExchangeEwsAccountSettingsStore};
use gpui_kit::*;
use uuid::Uuid;

use super::workspace::{AccountServices, AccountView, SetupIssue};
use crate::features::{
    bind_calendar_event_navigation_keys, bind_event_composer_keys, bind_mail_composer_keys,
    bind_message_navigation_keys,
};

const CLIENT_ID_ENV: &str = "EINFACH_MAIL_ENTRA_CLIENT_ID";
const CALLBACK_PORT_ENV: &str = "EINFACH_MAIL_ENTRA_CALLBACK_PORT";

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

pub(crate) fn run_application() -> Result<(), std::io::Error> {
    let (window_open_attempt_sender, window_open_attempt_receiver) =
        mpsc::channel::<std::io::Result<()>>();

    application().with_assets(assets::Assets).run(move |cx| {
        init(cx);
        bind_message_navigation_keys(cx);
        bind_mail_composer_keys(cx);
        bind_event_composer_keys(cx);
        bind_calendar_event_navigation_keys(cx);
        let account_services = configured_account_services();

        let window_open_attempt = open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|cx| AccountView::new(account_services, cx))
        })
        .map(|_| ())
        .map_err(|error| {
            std::io::Error::other(format!("failed to open the Einfach Mail window: {error:#}"))
        });

        if window_open_attempt.is_err() {
            cx.quit();
        }

        window_open_attempt_sender
            .send(window_open_attempt)
            .expect("the main thread keeps the receiver alive until GPUI exits");
    });

    match window_open_attempt_receiver.try_recv() {
        Ok(window_open_attempt) => window_open_attempt,
        Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => Err(
            std::io::Error::other("GPUI exited before opening the Einfach Mail window"),
        ),
    }
}

#[cfg(test)]
mod tests;
