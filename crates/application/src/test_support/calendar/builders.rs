pub(super) fn creation(view: CalendarViewReference) -> CalendarEventCreation {
    let local_start = PrimitiveDateTime::new(
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic date is valid"),
        Time::from_hms(9, 30, 0).expect("synthetic time is valid"),
    );
    let local_end = PrimitiveDateTime::new(
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic date is valid"),
        Time::from_hms(10, 0, 0).expect("synthetic time is valid"),
    );
    let draft = einfach_mail_domain::CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        local_start,
        local_end,
        "Etc/UTC"
            .to_owned()
            .try_into()
            .expect("synthetic time zone is valid"),
        utc(2030, Month::January, 8) + time::Duration::hours(9) + time::Duration::minutes(30),
        utc(2030, Month::January, 8) + time::Duration::hours(10),
    )
    .expect("synthetic event draft is valid");
    CalendarEventCreation::try_new(view, draft)
        .expect("synthetic event endpoints fit the selected week")
}

fn updated_event(
    update: &CalendarEventUpdate,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let source = update.source().occurrence();
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(update.draft().subject().to_owned()));
    let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
        return Err(CalendarOperationError::InvalidResponse);
    };
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(*start);
    time.set_end(*end);
    time.set_is_all_day(false);
    patch.set_time(time);
    let mut event = source.clone();
    patch
        .apply_to(&mut event)
        .map_err(|_| CalendarOperationError::InvalidResponse)?;
    Ok(event)
}

pub(super) fn update_request(event: EditableCalendarEvent) -> CalendarEventUpdate {
    let draft = CalendarEventUpdateDraft::timed(
        "Updated planning".to_owned(),
        utc(2030, Month::January, 8) + time::Duration::hours(11),
        utc(2030, Month::January, 8) + time::Duration::hours(12),
    )
    .expect("synthetic edit draft is valid");
    CalendarEventUpdate::new(event, draft).expect("synthetic event values changed")
}

pub(super) struct CalendarWorkflowPair {
    pub(super) first: Arc<CalendarWorkflow<FixtureProvider, FixtureCache>>,
    pub(super) sibling: Arc<CalendarWorkflow<FixtureProvider, FixtureCache>>,
}

pub(super) fn workflows_sharing_account_state(
    provider: &FixtureProvider,
    cache: &FixtureCache,
) -> CalendarWorkflowPair {
    let reservations = OperationReservations::default();
    let recovery = Arc::new(AccountWorkflowRecovery::default());
    let first = Arc::new(CalendarWorkflow::with_reservations(
        workflow_account_id(),
        provider.clone(),
        cache.clone(),
        reservations.clone(),
        Arc::clone(&recovery),
    ));
    let second = Arc::new(CalendarWorkflow::with_reservations(
        workflow_account_id(),
        provider.clone(),
        cache.clone(),
        reservations,
        recovery,
    ));
    CalendarWorkflowPair {
        first,
        sibling: second,
    }
}

fn apply_incremental_changes(
    events: &mut HashMap<String, CalendarOccurrence>,
    view: &CalendarViewReference,
    changes: Vec<CalendarDeltaChange>,
) -> Result<(), CalendarCacheError> {
    for change in changes {
        match change {
            CalendarDeltaChange::Removed { id } => {
                events.remove(id.as_str());
            }
            CalendarDeltaChange::Upsert { id, patch } => {
                let occurrence =
                    updated_occurrence(view, id.clone(), patch, events.remove(id.as_str()))?;
                if occurrence.overlaps_view() {
                    events.insert(id.as_str().to_owned(), occurrence);
                }
            }
        }
    }
    Ok(())
}

fn apply_replacement_changes(
    cached: &mut CachedView,
    view: &CalendarViewReference,
    changes: Vec<CalendarDeltaChange>,
) -> Result<(), CalendarCacheError> {
    let CachedView {
        events,
        replacement_events,
        removed_in_replacement,
        ..
    } = cached;
    let replacement_events = replacement_events
        .as_mut()
        .ok_or(CalendarCacheError::InvalidData)?;
    for change in changes {
        match change {
            CalendarDeltaChange::Removed { id } => {
                replacement_events.remove(id.as_str());
                removed_in_replacement.insert(id.as_str().to_owned());
            }
            CalendarDeltaChange::Upsert { id, patch } => {
                let existing = replacement_events.remove(id.as_str()).or_else(|| {
                    (!removed_in_replacement.contains(id.as_str()))
                        .then(|| events.get(id.as_str()).cloned())
                        .flatten()
                });
                let occurrence = updated_occurrence(view, id.clone(), patch, existing)?;
                removed_in_replacement.remove(id.as_str());
                replacement_events.insert(id.as_str().to_owned(), occurrence);
            }
        }
    }
    Ok(())
}

fn updated_occurrence(
    view: &CalendarViewReference,
    id: EventId,
    patch: CalendarOccurrencePatch,
    existing: Option<CalendarOccurrence>,
) -> Result<CalendarOccurrence, CalendarCacheError> {
    match existing {
        Some(mut occurrence) => {
            patch
                .apply_to(&mut occurrence)
                .map_err(|_| CalendarCacheError::InvalidData)?;
            Ok(occurrence)
        }
        None => patch
            .into_new(view.clone(), id)
            .map_err(|_| CalendarCacheError::InvalidData),
    }
}

pub(super) fn view() -> CalendarViewReference {
    let account = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account identifier is valid");
    let start_date = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic view start date is valid");
    let end_date = Date::from_calendar_date(2030, Month::January, 14)
        .expect("synthetic view end date is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic week range is valid");
    CalendarViewReference::new(account, range)
}

pub(super) fn workflow_account_id() -> AccountId {
    view().account_id().clone()
}

pub(super) fn event_id() -> EventId {
    EventId::try_from("synthetic-event".to_owned()).expect("synthetic event ID is valid")
}

pub(super) fn replacement_event_id() -> EventId {
    EventId::try_from("synthetic-replacement-event".to_owned())
        .expect("synthetic replacement event ID is valid")
}

pub(super) fn all_day_patch(
    subject: Option<&str>,
    location: Option<&str>,
) -> CalendarOccurrencePatch {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(subject.map(str::to_owned));
    patch.set_location(location.map(str::to_owned));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(utc(2030, Month::January, 8));
    time.set_end(utc(2030, Month::January, 10));
    time.set_is_all_day(true);
    patch.set_time(time);
    patch
}

pub(super) fn utc(year: i32, month: Month, day: u8) -> OffsetDateTime {
    Date::from_calendar_date(year, month, day)
        .expect("synthetic date is valid")
        .midnight()
        .assume_utc()
}

pub(super) fn link(cursor_token: &str) -> CalendarDeltaLink {
    CalendarDeltaLink::from_opaque(format!(
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken={cursor_token}"
    ))
}

pub(super) fn response(
    changes: Vec<CalendarDeltaChange>,
    progress: CalendarDeltaProgress,
) -> CalendarDeltaResponse {
    CalendarDeltaResponse::Page(CalendarDeltaPageResponse::new(
        CalendarDeltaPage::new(changes, progress),
        128,
    ))
}

pub(super) async fn synchronize_empty_view(
    workflow: &CalendarWorkflow<FixtureProvider, FixtureCache>,
    provider: &FixtureProvider,
    view: &CalendarViewReference,
    cursor: &str,
) {
    provider.push_page(Ok(response(
        Vec::new(),
        CalendarDeltaProgress::Complete(link(cursor)),
    )));
    workflow
        .sync_view(view)
        .await
        .expect("the synthetic calendar view completes its sync");
}

pub(super) fn cross_week_timed_event_patch() -> CalendarOccurrencePatch {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Cross-week appointment".to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(utc(2030, Month::January, 13) + time::Duration::hours(9));
    time.set_end(utc(2030, Month::January, 15) + time::Duration::hours(10));
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
}

pub(super) fn view_for_week(start_day: u8) -> CalendarViewReference {
    let account = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account identifier is valid");
    let start_date = Date::from_calendar_date(2030, Month::January, start_day)
        .expect("synthetic view start date is valid");
    let end_date = start_date
        .checked_add(time::Duration::days(7))
        .expect("synthetic view end date is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic calendar view is valid");
    CalendarViewReference::new(account, range)
}
