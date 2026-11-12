#[tokio::test]
async fn ews_provider_requires_a_session_for_create_and_recovery() {
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
        Err(CalendarOperationError::SignInRequired)
    ));
    assert!(matches!(
        provider.recover_event_creation(&creation).await,
        Err(CalendarOperationError::SignInRequired)
    ));
    assert!(matches!(
        provider.load_versioned_event(editable.occurrence()).await,
        Err(CalendarOperationError::UnsupportedOperation)
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
