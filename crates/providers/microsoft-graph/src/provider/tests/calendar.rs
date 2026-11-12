

#[tokio::test]
async fn create_and_recover_reuse_the_same_timed_primary_calendar_creation() {
    let server = MockServer::start_async().await;
    let token_refresh = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/token")
                .body_includes("grant_type=refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"token_type":"Bearer","access_token":"synthetic-calendar-access","expires_in":3600,"refresh_token":"synthetic-calendar-refresh","scope":"Calendars.ReadWrite Mail.Read User.Read offline_access"}"#,
                );
        })
        .await;
    let profile_request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me")
                .header("authorization", "Bearer synthetic-calendar-access");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"account-calendar","displayName":"Synthetic Calendar Account"}"#);
        })
        .await;
    let date =
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic event date is valid");
    let start = date
        .with_hms(9, 30, 0)
        .expect("synthetic event start is valid")
        .assume_utc();
    let end = date
        .with_hms(10, 0, 0)
        .expect("synthetic event end is valid")
        .assume_utc();
    let start_date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic week start is valid");
    let end_date =
        Date::from_calendar_date(2030, Month::January, 14).expect("synthetic week end is valid");
    let account =
        AccountId::try_from("account-calendar".to_owned()).expect("synthetic account ID is valid");
    let view = CalendarViewReference::new(
        account.clone(),
        CalendarViewRange::new(
            start_date.midnight().assume_utc(),
            end_date.midnight().assume_utc(),
            start_date,
            end_date,
        )
        .expect("synthetic calendar view is valid"),
    );
    let draft = CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        PrimitiveDateTime::new(
            date,
            Time::from_hms(9, 30, 0).expect("synthetic local start is valid"),
        ),
        PrimitiveDateTime::new(
            date,
            Time::from_hms(10, 0, 0).expect("synthetic local end is valid"),
        ),
        "Etc/UTC"
            .to_owned()
            .try_into()
            .expect("synthetic source zone is valid"),
        start,
        end,
    )
    .expect("synthetic event draft is valid");
    let creation = CalendarEventCreation::try_new(view.clone(), draft)
        .expect("synthetic event endpoints fit the selected week");
    let event_request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/calendar/events")
                .header("authorization", "Bearer synthetic-calendar-access")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"")
                .body_includes(r#""subject":"Synthetic planning meeting""#)
                .body_includes(r#""dateTime":"2030-01-08T09:30:00""#)
                .body_includes(r#""dateTime":"2030-01-08T10:00:00""#)
                .body_includes(r#""timeZone":"UTC""#)
                .body_includes(format!("\"transactionId\":\"{}\"", creation.id().as_uuid()));
            then.status(201)
                .header("content-type", "application/json")
                .body(format!(
                    r#"{{"id":"synthetic-created-event","subject":"Synthetic planning meeting","start":{{"dateTime":"2030-01-08T09:30:00","timeZone":"UTC"}},"end":{{"dateTime":"2030-01-08T10:00:00","timeZone":"UTC"}},"isAllDay":false,"transactionId":"{}"}}"#,
                    creation.id().as_uuid()
                ));
        })
        .await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
        "synthetic-calendar-refresh",
    ))));
    let provider = synthetic_provider(&server, browser_sender, credentials);
    provider
        .restore_account()
        .await
        .expect("synthetic account restores")
        .expect("synthetic account is connected");

    let event = provider
        .create_event(&creation)
        .await
        .expect("delegated Graph event creation succeeds");
    let recovered_event = provider
        .recover_event_creation(&creation)
        .await
        .expect("Graph recovery replays the operation with its transaction ID");
    assert_eq!(recovered_event, event);
    assert_eq!(event.reference(), &view);
    assert_eq!(event.subject(), Some("Synthetic planning meeting"));
    assert_eq!(event.time(), creation.draft().time());
    assert!(matches!(event.time(), CalendarOccurrenceTime::Timed { .. }));
    token_refresh.assert_async().await;
    profile_request.assert_async().await;
    event_request.assert_calls_async(2).await;
}

#[tokio::test]
async fn create_event_rejects_a_view_for_another_signed_in_account_before_posting() {
    let server = MockServer::start_async().await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
        "synthetic-calendar-refresh",
    ))));
    let provider = synthetic_provider(&server, browser_sender, credentials);
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: AccountId::try_from("signed-in-calendar-account".to_owned())
            .expect("synthetic signed-in account ID is valid"),
        access_token: Some(access_token("synthetic-calendar-access")),
    });
    let date =
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic event date is valid");
    let start_date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic week start is valid");
    let end_date =
        Date::from_calendar_date(2030, Month::January, 14).expect("synthetic week end is valid");
    let account = AccountId::try_from("other-calendar-account".to_owned())
        .expect("synthetic foreign account ID is valid");
    let view = CalendarViewReference::new(
        account,
        CalendarViewRange::new(
            start_date.midnight().assume_utc(),
            end_date.midnight().assume_utc(),
            start_date,
            end_date,
        )
        .expect("synthetic calendar view is valid"),
    );
    let draft = CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        PrimitiveDateTime::new(
            date,
            Time::from_hms(9, 30, 0).expect("synthetic local start is valid"),
        ),
        PrimitiveDateTime::new(
            date,
            Time::from_hms(10, 0, 0).expect("synthetic local end is valid"),
        ),
        "Etc/UTC"
            .to_owned()
            .try_into()
            .expect("synthetic source zone is valid"),
        date.with_hms(9, 30, 0)
            .expect("synthetic UTC start is valid")
            .assume_utc(),
        date.with_hms(10, 0, 0)
            .expect("synthetic UTC end is valid")
            .assume_utc(),
    )
    .expect("synthetic event draft is valid");
    let creation = CalendarEventCreation::try_new(view, draft)
        .expect("synthetic event endpoints fit the selected week");

    assert_eq!(
        provider.create_event(&creation).await,
        Err(einfach_mail_application::CalendarOperationError::AccountMismatch)
    );
}
