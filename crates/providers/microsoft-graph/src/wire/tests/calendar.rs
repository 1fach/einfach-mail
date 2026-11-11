

#[test]
fn create_event_request_contains_only_timed_primary_calendar_fields() {
    let creation = synthetic_calendar_creation();

    let body = create_calendar_event_body(&creation).expect("synthetic event body serializes");
    let body: serde_json::Value =
        serde_json::from_slice(&body).expect("synthetic event body is JSON");
    let object = body
        .as_object()
        .expect("synthetic event body is a JSON object");
    let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();

    assert_eq!(keys, ["end", "start", "subject", "transactionId"]);
    assert_eq!(body["subject"], "Synthetic planning meeting");
    assert_eq!(body["start"]["dateTime"], "2030-01-08T09:30:00");
    assert_eq!(body["start"]["timeZone"], "UTC");
    assert_eq!(body["end"]["dateTime"], "2030-01-08T10:00:00");
    assert_eq!(body["end"]["timeZone"], "UTC");
    assert_eq!(body["transactionId"], creation.id().as_uuid().to_string());
}

#[tokio::test]
async fn create_event_requires_a_matching_transaction_id_in_the_response() {
    let creation = synthetic_calendar_creation();
    for transaction_id in [None, Some("synthetic-other-operation")] {
        let mut response = json!({
            "id": "synthetic-created-event",
            "subject": "Synthetic planning meeting",
            "start": {"dateTime": "2030-01-08T09:30:00", "timeZone": "UTC"},
            "end": {"dateTime": "2030-01-08T10:00:00", "timeZone": "UTC"},
            "isAllDay": false
        });
        if let Some(transaction_id) = transaction_id {
            response["transactionId"] = json!(transaction_id);
        }
        assert_create_event_response_is_unknown(&creation, response).await;
    }
}

#[test]
fn event_update_body_contains_only_changed_fields() {
    let subject_update = CalendarEventUpdate::new(
        editable_event("synthetic-version-1"),
        CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc_time(8, 9),
            utc_time(8, 10),
        )
        .expect("synthetic subject update is valid"),
    )
    .expect("synthetic subject changed");
    let subject_body: serde_json::Value = serde_json::from_slice(
        &calendar_event_update_body(&subject_update).expect("synthetic subject update serializes"),
    )
    .expect("subject update is JSON");
    assert_eq!(subject_body, json!({"subject": "Updated planning"}));

    let time_update = CalendarEventUpdate::new(
        editable_event("synthetic-version-1"),
        CalendarEventUpdateDraft::timed("Planning".to_owned(), utc_time(8, 11), utc_time(8, 12))
            .expect("synthetic time update is valid"),
    )
    .expect("synthetic time changed");
    let time_body: serde_json::Value = serde_json::from_slice(
        &calendar_event_update_body(&time_update).expect("synthetic time update serializes"),
    )
    .expect("time update is JSON");
    assert_eq!(
        time_body,
        json!({
            "start": {"dateTime": "2030-01-08T11:00:00", "timeZone": "UTC"},
            "end": {"dateTime": "2030-01-08T12:00:00", "timeZone": "UTC"}
        })
    );
}

#[test]
fn event_edit_boundary_rejects_meetings_recurring_events_and_unknown_metadata() {
    let valid_event_json = editable_event_json(
        "synthetic-version-1",
        "Planning",
        "2030-01-08T09:00:00.0000000",
        "2030-01-08T10:00:00.0000000",
    );
    for event_json in [
        {
            let mut event_json = valid_event_json.clone();
            event_json["type"] = json!("occurrence");
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["isOrganizer"] = json!(false);
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["attendees"] = json!([{}]);
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["isOnlineMeeting"] = json!(true);
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["isAllDay"] = json!(true);
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["subject"] = serde_json::Value::Null;
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["subject"] = json!("");
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["subject"] = json!("Planning\tmeeting");
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json
                .as_object_mut()
                .expect("synthetic event is an object")
                .remove("changeKey");
            event_json
        },
    ] {
        let wire = serde_json::from_value::<WireCalendarEvent>(event_json)
            .expect("synthetic event metadata deserializes");
        assert_eq!(
            wire.into_editable(synthetic_view()),
            Err(GraphError::CalendarEventNotEditable)
        );
    }
}

#[tokio::test]
async fn event_update_rechecks_version_and_patches_only_requested_fields() {
    let server = MockServer::start_async().await;
    let view = synthetic_view();
    let source = serde_json::from_value::<WireCalendarEvent>(editable_event_json(
        "synthetic-version-1",
        "Planning",
        "2030-01-08T09:00:00.0000000",
        "2030-01-08T10:00:00.0000000",
    ))
    .expect("synthetic event deserializes")
    .into_new(view)
    .expect("synthetic cached event is complete");
    let updated = editable_event_json(
        "synthetic-version-2",
        "Updated planning",
        "2030-01-08T11:00:00.0000000",
        "2030-01-08T12:00:00.0000000",
    );
    let read = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"")
                .query_param("$select", CALENDAR_EVENT_EDIT_SELECT);
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    editable_event_json(
                        "synthetic-version-1",
                        "Planning",
                        "2030-01-08T09:00:00.0000000",
                        "2030-01-08T10:00:00.0000000",
                    )
                    .to_string(),
                );
        })
        .await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"")
                .query_param_missing("$select")
                .header("content-type", "application/json")
                .body(r#"{"subject":"Updated planning","start":{"dateTime":"2030-01-08T11:00:00","timeZone":"UTC"},"end":{"dateTime":"2030-01-08T12:00:00","timeZone":"UTC"}}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(updated.to_string());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let editable = api
        .editable_calendar_event(&source, synthetic_access_token())
        .await
        .expect("synthetic supported event passes the edit check");
    let update = CalendarEventUpdate::new(
        editable,
        CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc_time(8, 11),
            utc_time(8, 12),
        )
        .expect("synthetic update draft is valid"),
    )
    .expect("synthetic event values changed");

    let saved = api
        .update_calendar_event(&update, synthetic_access_token())
        .await
        .expect("Graph confirms the exact event update");

    assert_eq!(saved.subject(), Some("Updated planning"));
    assert_eq!(saved.location(), Some("Room A"));
    assert_eq!(saved.time(), update.draft().time());
    read.assert_calls_async(2).await;
    patch.assert_async().await;
}

#[tokio::test]
async fn event_update_refuses_a_changed_version_without_patching() {
    let server = MockServer::start_async().await;
    let source = editable_event("synthetic-version-1");
    let update = CalendarEventUpdate::new(
        source,
        CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc_time(8, 11),
            utc_time(8, 12),
        )
        .expect("synthetic update draft is valid"),
    )
    .expect("synthetic event values changed");
    let read = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me/events/synthetic-event");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    editable_event_json(
                        "synthetic-version-2",
                        "Concurrent change",
                        "2030-01-08T09:00:00.0000000",
                        "2030-01-08T10:00:00.0000000",
                    )
                    .to_string(),
                );
        })
        .await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH).path("/v1.0/me/events/synthetic-event");
            then.status(200).body("{}");
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    assert_eq!(
        api.update_calendar_event(&update, synthetic_access_token())
            .await,
        Err(GraphError::CalendarEventChanged)
    );
    read.assert_calls_async(1).await;
    patch.assert_calls_async(0).await;
}

#[tokio::test]
async fn event_update_refuses_a_missing_event_without_patching() {
    let server = MockServer::start_async().await;
    let update = CalendarEventUpdate::new(
        editable_event("synthetic-version-1"),
        CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc_time(8, 11),
            utc_time(8, 12),
        )
        .expect("synthetic update draft is valid"),
    )
    .expect("synthetic event values changed");
    let read = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me/events/synthetic-event");
            then.status(404);
        })
        .await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH).path("/v1.0/me/events/synthetic-event");
            then.status(200).body("{}");
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    assert_eq!(
        api.update_calendar_event(&update, synthetic_access_token())
            .await,
        Err(GraphError::CalendarEventChanged)
    );
    read.assert_calls_async(1).await;
    patch.assert_calls_async(0).await;
}

#[tokio::test]
async fn event_delete_rechecks_version_and_sends_the_current_etag() {
    let server = MockServer::start_async().await;
    let event = editable_event("synthetic-version-1");
    let read = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"")
                .query_param("$select", CALENDAR_EVENT_EDIT_SELECT);
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    editable_event_json(
                        "synthetic-version-1",
                        "Planning",
                        "2030-01-08T09:00:00.0000000",
                        "2030-01-08T10:00:00.0000000",
                    )
                    .to_string(),
                );
        })
        .await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"")
                .header("if-match", "W/\"synthetic-etag-1\"");
            then.status(204);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    api.delete_calendar_event(&event, synthetic_access_token())
        .await
        .expect("Graph confirms the conditional delete");

    read.assert_calls_async(1).await;
    delete.assert_calls_async(1).await;
}

#[tokio::test]
async fn event_delete_refuses_a_changed_version_without_deleting() {
    let server = MockServer::start_async().await;
    let event = editable_event("synthetic-version-1");
    let read = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me/events/synthetic-event");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    editable_event_json(
                        "synthetic-version-2",
                        "Changed planning",
                        "2030-01-08T09:00:00.0000000",
                        "2030-01-08T10:00:00.0000000",
                    )
                    .to_string(),
                );
        })
        .await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE).path("/v1.0/me/events/synthetic-event");
            then.status(204);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    assert_eq!(
        api.delete_calendar_event(&event, synthetic_access_token())
            .await,
        Err(GraphError::CalendarEventChanged)
    );
    read.assert_calls_async(1).await;
    delete.assert_calls_async(0).await;
}

#[tokio::test]
async fn event_delete_requires_an_etag_before_sending() {
    let server = MockServer::start_async().await;
    let event = editable_event("synthetic-version-1");
    let mut current = editable_event_json(
        "synthetic-version-1",
        "Planning",
        "2030-01-08T09:00:00.0000000",
        "2030-01-08T10:00:00.0000000",
    );
    current
        .as_object_mut()
        .expect("synthetic event JSON is an object")
        .remove("@odata.etag");
    let read = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me/events/synthetic-event");
            then.status(200)
                .header("content-type", "application/json")
                .body(current.to_string());
        })
        .await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE).path("/v1.0/me/events/synthetic-event");
            then.status(204);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    assert_eq!(
        api.delete_calendar_event(&event, synthetic_access_token())
            .await,
        Err(GraphError::InvalidResponse)
    );
    read.assert_calls_async(1).await;
    delete.assert_calls_async(0).await;
}

#[tokio::test]
async fn calendar_delta_uses_exact_week_bounds_and_maps_sparse_followup_changes() {
    let server = MockServer::start_async().await;
    let next_link = http_url(
        &server,
        "/v1.0/me/calendarView/delta?$skiptoken=synthetic-next",
    );
    let final_link = http_url(
        &server,
        "/v1.0/me/calendarView/delta?$deltatoken=synthetic-final",
    );
    let initial_page = json!({
        "value": [{
            "id": "synthetic-event",
            "subject": "Synthetic all-day appointment",
            "location": {"displayName": "Room 4"},
            "start": {
                "dateTime": "2030-01-07T00:00:00.0000000",
                "timeZone": "UTC"
            },
            "end": {
                "dateTime": "2030-01-08T00:00:00.0000000",
                "timeZone": "UTC"
            },
            "isAllDay": true,
            "originalStartTimeZone": "Pacific Standard Time",
            "originalEndTimeZone": "Pacific Standard Time"
        }],
        "@odata.nextLink": next_link.clone()
    });
    let continuation_page = json!({
        "value": [{
            "id": "synthetic-event",
            "location": null
        }],
        "@odata.deltaLink": final_link.clone()
    });
    let first_request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("startDateTime", "2030-01-07T00:00:00Z")
                .query_param("endDateTime", "2030-01-14T00:00:00Z")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.status(200)
                .header("content-type", "application/json")
                .body(initial_page.to_string());
        })
        .await;
    let continuation_request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("$skiptoken", "synthetic-next")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.status(200)
                .header("content-type", "application/json")
                .body(continuation_page.to_string());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let view = synthetic_view();
    let token = synthetic_access_token();

    let initial = api
        .calendar_delta_page(&view, CalendarDeltaRequest::Initial, Arc::clone(&token))
        .await
        .expect("synthetic bounded calendar delta request succeeds");
    let CalendarDeltaResponse::Page(initial) = initial else {
        panic!("initial request returns a page");
    };
    let (initial_page, initial_response_bytes) = initial.into_parts();
    assert!(initial_response_bytes > 0);
    let (mut changes, progress) = initial_page.into_parts();
    let next = match progress {
        CalendarDeltaProgress::Next(link) => link,
        CalendarDeltaProgress::Complete(_) => panic!("initial response must have a next link"),
    };
    assert_eq!(next.as_str(), next_link);
    let CalendarDeltaChange::Upsert { id, patch } = changes.remove(0) else {
        panic!("synthetic event must be an upsert");
    };
    let mut occurrence: CalendarOccurrence = patch
        .into_new(view.clone(), id)
        .expect("new Graph occurrence includes a complete date range");
    assert_eq!(occurrence.subject(), Some("Synthetic all-day appointment"));
    assert_eq!(occurrence.location(), Some("Room 4"));
    assert_eq!(
        occurrence.original_start_time_zone(),
        Some("Pacific Standard Time")
    );
    assert!(matches!(
        occurrence.time(),
        CalendarOccurrenceTime::AllDay { start, end_exclusive }
            if *start == Date::from_calendar_date(2030, Month::January, 7).expect("valid date")
                && *end_exclusive == Date::from_calendar_date(2030, Month::January, 8).expect("valid date")
    ));

    let continuation = api
        .calendar_delta_page(&view, CalendarDeltaRequest::Continue(next), token)
        .await
        .expect("synthetic continuation request succeeds");
    let CalendarDeltaResponse::Page(continuation) = continuation else {
        panic!("continuation request returns a page");
    };
    let (continuation_page, _) = continuation.into_parts();
    let (mut changes, progress) = continuation_page.into_parts();
    assert!(
        matches!(progress, CalendarDeltaProgress::Complete(ref link) if link.as_str() == final_link)
    );
    let CalendarDeltaChange::Upsert { patch, .. } = changes.remove(0) else {
        panic!("synthetic sparse update must be an upsert");
    };
    patch
        .apply_to(&mut occurrence)
        .expect("sparse update preserves the existing occurrence range");
    assert_eq!(occurrence.subject(), Some("Synthetic all-day appointment"));
    assert_eq!(occurrence.location(), None);
    assert!(matches!(
        occurrence.time(),
        CalendarOccurrenceTime::AllDay { .. }
    ));

    first_request.assert_async().await;
    continuation_request.assert_async().await;
}

#[tokio::test]
async fn calendar_delta_preserves_recurring_occurrence_instants_across_dst() {
    let server = MockServer::start_async().await;
    let delta_link = http_url(
        &server,
        "/v1.0/me/calendarView/delta?$deltatoken=synthetic-dst",
    );
    let page = json!({
        "value": [
            recurring_graph_event_json(
                "synthetic-before-dst",
                "2026-03-22T08:00:00.0000000",
                "2026-03-22T09:00:00.0000000"
            ),
            recurring_graph_event_json(
                "synthetic-after-dst",
                "2026-03-29T07:00:00.0000000",
                "2026-03-29T08:00:00.0000000"
            )
        ],
        "@odata.deltaLink": delta_link
    });
    let request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("startDateTime", "2026-03-15T23:00:00Z")
                .query_param("endDateTime", "2026-03-29T22:00:00Z")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.status(200)
                .header("content-type", "application/json")
                .body(page.to_string());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let start_date = Date::from_calendar_date(2026, Month::March, 16)
        .expect("synthetic calendar start date is valid");
    let end_date = Date::from_calendar_date(2026, Month::March, 30)
        .expect("synthetic calendar end date is valid");
    let range = CalendarViewRange::new(
        synthetic_utc_datetime("2026-03-15T23:00:00Z"),
        synthetic_utc_datetime("2026-03-29T22:00:00Z"),
        start_date,
        end_date,
    )
    .expect("synthetic calendar range spans the DST transition");
    let view = CalendarViewReference::new(account, range);

    let response = api
        .calendar_delta_page(
            &view,
            CalendarDeltaRequest::Initial,
            synthetic_access_token(),
        )
        .await
        .expect("synthetic calendar delta request succeeds");
    let CalendarDeltaResponse::Page(page) = response else {
        panic!("synthetic initial calendar response contains a page");
    };
    let (page, response_bytes) = page.into_parts();
    assert!(response_bytes > 0, "Graph returns a non-empty delta page");
    let (changes, progress) = page.into_parts();
    assert!(
        matches!(progress, CalendarDeltaProgress::Complete(_)),
        "synthetic initial response ends with a delta link"
    );
    assert_eq!(changes.len(), 2, "Graph expands both weekly occurrences");
    let mut changes = changes.into_iter();
    let before_dst = occurrence_from_graph_change(
        changes
            .next()
            .expect("the response contains a pre-DST occurrence"),
        &view,
    );
    let after_dst = occurrence_from_graph_change(
        changes
            .next()
            .expect("the response contains a post-DST occurrence"),
        &view,
    );
    assert!(
        changes.next().is_none(),
        "the response contains two occurrences"
    );

    assert_dst_occurrence(
        &before_dst,
        "synthetic-before-dst",
        "2026-03-22T08:00:00Z",
        "2026-03-22T09:00:00Z",
    );
    assert_dst_occurrence(
        &after_dst,
        "synthetic-after-dst",
        "2026-03-29T07:00:00Z",
        "2026-03-29T08:00:00Z",
    );
    request.assert_async().await;
}

#[tokio::test]
async fn calendar_410_reset_requires_a_valid_location_on_a_continuation() {
    let server = MockServer::start_async().await;
    let reset_link = format!(
        "{}/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=",
        http_base_url(&server)
    );
    let mock_reset_link = reset_link.clone();
    let expired = server
        .mock_async(move |when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("$deltatoken", "expired")
                .header("content-type", "application/json");
            then.status(410).header("Location", mock_reset_link.clone());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let response = api
        .calendar_delta_page(
            &synthetic_view(),
            CalendarDeltaRequest::Continue(CalendarDeltaLink::from_opaque(format!(
                "{}/v1.0/me/calendarView/delta?$deltatoken=expired",
                http_base_url(&server)
            ))),
            synthetic_access_token(),
        )
        .await
        .expect("documented 410 reset URL is returned after validation");

    assert_eq!(
        response,
        CalendarDeltaResponse::Reset {
            reset_link: Some(CalendarDeltaLink::from_opaque(reset_link.clone())),
            response_bytes: reset_link.len(),
        }
    );
    expired.assert_async().await;
}

#[tokio::test]
async fn sync_state_not_found_starts_the_fixed_range_without_a_location() {
    let server = MockServer::start_async().await;
    let body = r#"{"error":{"code":"syncStateNotFound"}}"#;
    let expired = server
        .mock_async(move |when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("$deltatoken", "expired")
                .header("content-type", "application/json");
            then.status(400).body(body);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let response = api
        .calendar_delta_page(
            &synthetic_view(),
            CalendarDeltaRequest::Continue(CalendarDeltaLink::from_opaque(format!(
                "{}/v1.0/me/calendarView/delta?$deltatoken=expired",
                http_base_url(&server)
            ))),
            synthetic_access_token(),
        )
        .await
        .expect("documented expiration code is mapped without returning the error body");

    assert_eq!(
        response,
        CalendarDeltaResponse::Reset {
            reset_link: None,
            response_bytes: body.len(),
        }
    );
    expired.assert_async().await;
}

#[test]
fn rejects_non_utc_and_explicit_offset_calendar_times() {
    for (date_time, time_zone) in [
        ("2030-01-07T09:00:00.0000000", "Pacific Standard Time"),
        ("2030-01-07T09:00:00+01:00", "UTC"),
    ] {
        let event: WireCalendarEvent = serde_json::from_value(json!({
            "id": "synthetic-event",
            "start": {"dateTime": date_time, "timeZone": time_zone}
        }))
        .expect("synthetic event wire fields are valid JSON");
        assert_eq!(event.into_change(), Err(GraphError::InvalidRecord));
    }
}

#[test]
fn created_event_response_requires_a_complete_timed_range() {
    let event: WireCalendarEvent = serde_json::from_value(json!({
        "id": "synthetic-event",
        "subject": "Synthetic planning meeting",
        "isAllDay": false
    }))
    .expect("synthetic incomplete event is valid JSON");

    assert_eq!(
        event.into_new(synthetic_view()),
        Err(GraphError::InvalidRecord)
    );
}