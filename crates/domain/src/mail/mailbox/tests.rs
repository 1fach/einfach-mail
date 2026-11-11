use super::{EmptyMailboxAddress, Mailbox};

#[test]
fn mailbox_requires_a_non_empty_address() {
    for address in [String::new(), " \t\n".to_owned()] {
        assert_eq!(Mailbox::try_new(address, None), Err(EmptyMailboxAddress));
    }
}

#[test]
fn mailbox_debug_output_redacts_address_and_display_name() {
    let mailbox = Mailbox::try_new(
        "mailbox-address-marker@example.test".to_owned(),
        Some("mailbox-display-marker".to_owned()),
    )
    .expect("synthetic mailbox is valid");

    let debug = format!("{mailbox:?}");

    assert!(
        !debug.contains("mailbox-address-marker"),
        "Debug output included the mailbox address: {debug}"
    );
    assert!(
        !debug.contains("mailbox-display-marker"),
        "Debug output included the mailbox display name: {debug}"
    );
}
