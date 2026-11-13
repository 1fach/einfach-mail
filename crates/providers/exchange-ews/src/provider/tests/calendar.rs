#[tokio::test]
async fn ews_provider_rejects_unsupported_calendar_writes_before_session_lookup() {
    let provider = ExchangeEwsProvider::new(EmptySettingsStore);
    let view = synthetic_calendar_view();
    let creation = synthetic_event_creation(&view);
    let editable = synthetic_versioned_event(&view);
    let update_draft = CalendarEventUpdateDraft::timed(
        "Changed event".to_owned(),
        view.range().start() + time::Duration::hours(2),
        view.range().start() + time::Duration::hours(3),
    )
    .expect("synthetic event update is valid");
    let update = CalendarEventUpdate::new(editable.clone(), update_draft)
        .expect("synthetic update changes the event");

    assert!(matches!(
        provider.create_event(&creation).await,
        Err(CalendarOperationError::UnsupportedOperation)
    ));
    assert!(matches!(
        provider.recover_event_creation(&creation).await,
        Err(CalendarOperationError::SignInRequired)
    ));
    assert!(matches!(
        provider.load_versioned_event(editable.occurrence()).await,
        Err(CalendarOperationError::SignInRequired)
    ));
    assert!(matches!(
        provider.update_event(&update).await,
        Err(CalendarOperationError::UnsupportedOperation)
    ));
    assert!(matches!(
        provider.delete_event(&editable).await,
        Err(CalendarOperationError::UnsupportedOperation)
    ));
}

#[tokio::test]
async fn ews_provider_advertises_the_calendar_capabilities_it_enforces() {
    let provider = ExchangeEwsProvider::new(EmptySettingsStore);

    assert_eq!(
        provider.calendar_capabilities().await,
        Ok(CalendarCapabilities::view_only())
    );
}
