//! Composes the GPUI mail and calendar views with application workflows.

use std::{
    env::{self, VarError},
    sync::{Arc, mpsc},
};

use einfach_mail_application::{
    AccountSessionFactory, AccountSessionWorkflowFactory, AccountWorkflow,
};
use einfach_mail_microsoft_graph::{CallbackPort, MicrosoftGraphProvider};
use einfach_mail_persistence::SqliteMailCacheFactory;
use gpui_kit::*;
use uuid::Uuid;

mod account_view;
mod calendar_time;
mod calendar_view;
mod calendar_week;
mod event_composer;
mod list_navigation;
mod mail_composer;
mod mail_view;
mod message_body;
mod single_instance;
mod sync_age;
#[cfg(test)]
mod test_support;

use account_view::{AccountServices, AccountView, SetupIssue};

const CLIENT_ID_ENV: &str = "EINFACH_MAIL_ENTRA_CLIENT_ID";
const CALLBACK_PORT_ENV: &str = "EINFACH_MAIL_ENTRA_CALLBACK_PORT";

fn configured_account_services() -> Result<AccountServices<MicrosoftGraphProvider>, SetupIssue> {
    let client_id = match env::var(CLIENT_ID_ENV) {
        Ok(client_id_value) => {
            Uuid::parse_str(&client_id_value).map_err(|_| SetupIssue::InvalidClientId)?
        }
        Err(VarError::NotPresent) => return Err(SetupIssue::MissingClientId),
        Err(VarError::NotUnicode(_)) => return Err(SetupIssue::InvalidClientId),
    };
    let callback_port_value = match env::var(CALLBACK_PORT_ENV) {
        Ok(port_value) => Some(port_value),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(_)) => return Err(SetupIssue::InvalidCallbackPort),
    };
    let callback_port = configured_callback_port(callback_port_value.as_deref())?;

    let provider = MicrosoftGraphProvider::new(client_id, callback_port);
    let account_workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let account_session_factory: Arc<dyn AccountSessionFactory> = Arc::new(
        AccountSessionWorkflowFactory::new(provider, SqliteMailCacheFactory),
    );
    Ok(AccountServices::new(
        account_workflow,
        account_session_factory,
    ))
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

fn main() -> Result<(), std::io::Error> {
    single_instance::run_with_user_lock(run_application)?
}

fn run_application() -> Result<(), std::io::Error> {
    let (window_open_attempt_sender, window_open_attempt_receiver) =
        mpsc::channel::<std::io::Result<()>>();

    application().with_assets(assets::Assets).run(move |cx| {
        init(cx);
        mail_view::bind_message_navigation_keys(cx);
        mail_composer::bind_mail_composer_keys(cx);
        event_composer::bind_event_composer_keys(cx);
        calendar_view::bind_calendar_event_navigation_keys(cx);
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
mod tests {
    use super::{SetupIssue, configured_callback_port};
    use einfach_mail_microsoft_graph::CallbackPort;

    #[test]
    fn desktop_auth_requires_a_registered_fixed_loopback_port() {
        assert_eq!(
            configured_callback_port(None),
            Err(SetupIssue::MissingCallbackPort)
        );
        assert_eq!(
            configured_callback_port(Some("0")),
            Err(SetupIssue::InvalidCallbackPort)
        );
        assert_eq!(
            configured_callback_port(Some("invalid")),
            Err(SetupIssue::InvalidCallbackPort)
        );
        assert!(matches!(
            configured_callback_port(Some("43123")),
            Ok(CallbackPort::Fixed(port)) if port.get() == 43123
        ));
    }
}
