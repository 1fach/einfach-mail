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
