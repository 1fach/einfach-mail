use std::sync::Arc;

use huskarl::token::AccessToken;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::{
    error::GraphError,
    transport::{
        GraphCalendarDeltaOperation, GraphPatchOperation, GraphPostJsonOperation,
        GraphReadOperation, GraphResponse,
    },
};
use einfach_mail_application::{
    CalendarDeltaPageResponse, CalendarDeltaRequest, CalendarDeltaResponse,
};
use einfach_mail_domain::{
    CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarEventCreation, CalendarEventUpdate, CalendarEventVersion, CalendarOccurrence,
    CalendarOccurrencePatch, CalendarOccurrenceTime, CalendarOccurrenceTimePatch,
    CalendarViewReference, EditableCalendarEvent, EventId,
};

use super::{
    GraphApi,
    pagination::GraphDeltaPage,
    shared::{WireField, WireRemoved},
};

impl GraphApi {
    pub(crate) async fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        request: CalendarDeltaRequest,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<CalendarDeltaResponse, GraphError> {
        let (operation, request_url) = match request {
            CalendarDeltaRequest::Initial => (
                GraphCalendarDeltaOperation::Initial,
                self.authority.calendar_view_delta_path(view.range())?,
            ),
            CalendarDeltaRequest::Continue(link) => (
                GraphCalendarDeltaOperation::Continuation,
                self.authority.calendar_delta_opaque_link(link.as_str())?,
            ),
        };
        let response = self
            .transport
            .get_calendar_delta(operation, request_url, token)
            .await?;
        match response {
            GraphResponse::CalendarViewDeltaReset {
                location,
                response_bytes,
            } => {
                let reset_link = location
                    .map(|location| {
                        self.authority
                            .calendar_delta_reset_link(view.range(), &location)
                    })
                    .transpose()?
                    .map(|url| CalendarDeltaLink::from_opaque(url.as_str().to_owned()));
                Ok(CalendarDeltaResponse::Reset {
                    reset_link,
                    response_bytes,
                })
            }
            GraphResponse::Body(response) => {
                let response_bytes = response.len();
                let page = self.parse_calendar_delta_page(&response)?;
                Ok(CalendarDeltaResponse::Page(CalendarDeltaPageResponse::new(
                    page,
                    response_bytes,
                )))
            }
            GraphResponse::MessageDeltaReset(_) => Err(GraphError::InvalidResponse),
        }
    }

    pub(crate) async fn create_calendar_event(
        &self,
        creation: &CalendarEventCreation,
        token: Arc<AccessToken>,
    ) -> Result<CalendarOccurrence, GraphError> {
        let url = self.authority.primary_calendar_events_path()?;
        let body = create_calendar_event_body(creation)?;
        let response = self
            .transport
            .post_json(
                GraphPostJsonOperation::CalendarEventCreate,
                url,
                token,
                body,
            )
            .await?;
        let event = serde_json::from_slice::<WireCalendarEvent>(&response)
            .map_err(|_| GraphError::CalendarEventCreationOutcomeUnknown)?;
        event
            .into_new(creation.view().clone())
            .map_err(|_| GraphError::CalendarEventCreationOutcomeUnknown)
    }

    pub(crate) async fn editable_calendar_event(
        &self,
        event: &CalendarOccurrence,
        token: Arc<AccessToken>,
    ) -> Result<EditableCalendarEvent, GraphError> {
        let editable = self
            .fetch_editable_calendar_event(event, token)
            .await?
            .editable;
        if editable.occurrence().reference() != event.reference()
            || editable.occurrence().id() != event.id()
        {
            return Err(GraphError::InvalidRecord);
        }
        Ok(editable)
    }

    async fn fetch_editable_calendar_event(
        &self,
        event: &CalendarOccurrence,
        token: Arc<AccessToken>,
    ) -> Result<CheckedEditableCalendarEvent, GraphError> {
        let view = event.reference().clone();
        let url = self
            .authority
            .editable_calendar_event_path(event.id().as_str())?;
        let response = self
            .transport
            .get(GraphReadOperation::CalendarEventRead, url, token)
            .await?;
        let event = serde_json::from_slice::<WireCalendarEvent>(&response)
            .map_err(|_| GraphError::InvalidResponse)?;
        event.into_checked_editable(view)
    }

    pub(crate) async fn update_calendar_event(
        &self,
        update: &CalendarEventUpdate,
        token: Arc<AccessToken>,
    ) -> Result<CalendarOccurrence, GraphError> {
        let current = self
            .current_editable_calendar_event(update.source(), Arc::clone(&token))
            .await?;
        self.patch_calendar_event(update, &current.editable, token)
            .await
    }

    pub(crate) async fn delete_calendar_event(
        &self,
        event: &EditableCalendarEvent,
        token: Arc<AccessToken>,
    ) -> Result<(), GraphError> {
        let current = self
            .current_editable_calendar_event(event, Arc::clone(&token))
            .await?;
        let etag = current.etag.as_deref().ok_or(GraphError::InvalidResponse)?;
        let url = self
            .authority
            .calendar_event_path(event.occurrence().id().as_str())?;
        self.transport.delete_calendar_event(url, token, etag).await
    }

    async fn current_editable_calendar_event(
        &self,
        source: &EditableCalendarEvent,
        token: Arc<AccessToken>,
    ) -> Result<CheckedEditableCalendarEvent, GraphError> {
        let source_event = source.occurrence();
        let current = self
            .fetch_editable_calendar_event(source_event, Arc::clone(&token))
            .await
            .map_err(|error| match error {
                GraphError::CalendarEventNotEditable => GraphError::CalendarEventChanged,
                other => other,
            })?;
        if current.editable.occurrence().reference() != source_event.reference()
            || current.editable.occurrence().id() != source_event.id()
            || current.editable.version() != source.version()
        {
            return Err(GraphError::CalendarEventChanged);
        }
        Ok(current)
    }

    async fn patch_calendar_event(
        &self,
        update: &CalendarEventUpdate,
        current: &EditableCalendarEvent,
        token: Arc<AccessToken>,
    ) -> Result<CalendarOccurrence, GraphError> {
        let body = calendar_event_update_body(update)?;
        let event_id = update.source().occurrence().id();
        let response = self
            .transport
            .patch_json(
                GraphPatchOperation::CalendarEventUpdate,
                self.authority.calendar_event_path(event_id.as_str())?,
                token,
                body.into(),
            )
            .await?;
        confirm_calendar_event_update(update, current, &response)
    }

    fn parse_calendar_delta_page(&self, response: &[u8]) -> Result<CalendarDeltaPage, GraphError> {
        let page = serde_json::from_slice::<GraphDeltaPage<WireCalendarEvent>>(response)
            .map_err(|_| GraphError::InvalidResponse)?;
        let progress = self.calendar_delta_progress(page.next_link, page.delta_link)?;
        let changes = page
            .records
            .into_iter()
            .map(WireCalendarEvent::into_change)
            .collect::<Result<_, _>>()?;
        Ok(CalendarDeltaPage::new(changes, progress))
    }

    fn calendar_delta_progress(
        &self,
        next_link: Option<String>,
        delta_link: Option<String>,
    ) -> Result<CalendarDeltaProgress, GraphError> {
        match (next_link, delta_link) {
            (Some(link), None) => Ok(CalendarDeltaProgress::Next(CalendarDeltaLink::from_opaque(
                self.authority
                    .calendar_delta_opaque_link(&link)?
                    .as_str()
                    .to_owned(),
            ))),
            (None, Some(link)) => Ok(CalendarDeltaProgress::Complete(
                CalendarDeltaLink::from_opaque(
                    self.authority
                        .calendar_delta_opaque_link(&link)?
                        .as_str()
                        .to_owned(),
                ),
            )),
            _ => Err(GraphError::InvalidResponse),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireCalendarEvent {
    pub(super) id: String,
    #[serde(default)]
    pub(super) subject: WireField<String>,
    #[serde(default)]
    pub(super) location: WireField<WireLocation>,
    #[serde(default)]
    pub(super) start: WireField<WireDateTimeTimeZone>,
    #[serde(default)]
    pub(super) end: WireField<WireDateTimeTimeZone>,
    #[serde(default)]
    pub(super) is_all_day: WireField<bool>,
    #[serde(default)]
    pub(super) original_start_time_zone: WireField<String>,
    #[serde(default)]
    pub(super) original_end_time_zone: WireField<String>,
    #[serde(rename = "type")]
    pub(super) event_type: Option<String>,
    pub(super) is_organizer: Option<bool>,
    pub(super) attendees: Option<Vec<serde::de::IgnoredAny>>,
    pub(super) is_online_meeting: Option<bool>,
    pub(super) change_key: Option<String>,
    #[serde(rename = "@odata.etag")]
    pub(super) odata_etag: Option<String>,
    #[serde(rename = "@removed")]
    pub(super) removed: Option<WireRemoved>,
}

struct CheckedEditableCalendarEvent {
    editable: EditableCalendarEvent,
    etag: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireLocation {
    display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireDateTimeTimeZone {
    date_time: String,
    time_zone: String,
}

impl WireCalendarEvent {
    fn into_checked_editable(
        self,
        view: CalendarViewReference,
    ) -> Result<CheckedEditableCalendarEvent, GraphError> {
        let etag = self.odata_etag.clone();
        let editable = self.into_editable(view)?;
        Ok(CheckedEditableCalendarEvent { editable, etag })
    }

    pub(super) fn into_editable(
        self,
        view: CalendarViewReference,
    ) -> Result<EditableCalendarEvent, GraphError> {
        if !self.has_supported_editable_shape() {
            return Err(GraphError::CalendarEventNotEditable);
        }
        let version = self
            .change_key
            .clone()
            .ok_or(GraphError::CalendarEventNotEditable)?;
        let version =
            CalendarEventVersion::try_from(version).map_err(|_| GraphError::InvalidRecord)?;
        let occurrence = self.into_new(view)?;
        EditableCalendarEvent::new(occurrence, version).map_err(|_| GraphError::InvalidRecord)
    }

    fn has_supported_editable_shape(&self) -> bool {
        self.event_type.as_deref() == Some("singleInstance")
            && self.is_organizer == Some(true)
            && self.attendees.as_ref().is_some_and(Vec::is_empty)
            && self.is_online_meeting == Some(false)
            && matches!(self.is_all_day, WireField::Value(false))
            && matches!(
                &self.subject,
                WireField::Value(subject)
                    if !subject.trim().is_empty()
                        && !subject.chars().any(char::is_control)
            )
    }

    pub(super) fn into_change(self) -> Result<CalendarDeltaChange, GraphError> {
        let id = EventId::try_from(self.id).map_err(|_| GraphError::InvalidRecord)?;
        if self.removed.is_some() {
            return Ok(CalendarDeltaChange::Removed { id });
        }

        let mut patch = CalendarOccurrencePatch::new();
        apply_calendar_content_fields(self.subject, self.location, &mut patch);
        apply_calendar_zone_fields(
            self.original_start_time_zone,
            self.original_end_time_zone,
            &mut patch,
        );
        if let Some(time_patch) =
            calendar_occurrence_time_patch(self.start, self.end, self.is_all_day)?
        {
            patch.set_time(time_patch);
        }
        Ok(CalendarDeltaChange::Upsert { id, patch })
    }

    pub(super) fn into_new(
        self,
        view: CalendarViewReference,
    ) -> Result<CalendarOccurrence, GraphError> {
        match self.into_change()? {
            CalendarDeltaChange::Upsert { id, patch } => patch
                .into_new(view, id)
                .map_err(|_| GraphError::InvalidRecord),
            CalendarDeltaChange::Removed { .. } => Err(GraphError::InvalidRecord),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateCalendarEventRequest<'a> {
    subject: &'a str,
    start: CreateEventDateTimeTimeZone,
    end: CreateEventDateTimeTimeZone,
    transaction_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateEventDateTimeTimeZone {
    date_time: String,
    time_zone: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CalendarEventUpdateRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<CreateEventDateTimeTimeZone>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end: Option<CreateEventDateTimeTimeZone>,
}

pub(super) fn calendar_event_update_body(
    update: &CalendarEventUpdate,
) -> Result<Vec<u8>, GraphError> {
    let request = calendar_event_update_request(update)?;
    serde_json::to_vec(&request).map_err(|_| GraphError::InvalidRecord)
}

fn calendar_event_update_request(
    update: &CalendarEventUpdate,
) -> Result<CalendarEventUpdateRequest<'_>, GraphError> {
    let source = update.source().occurrence();
    let subject =
        (source.subject() != Some(update.draft().subject())).then_some(update.draft().subject());
    let (start, end) = calendar_event_update_times(update, source)?;
    if subject.is_none() && start.is_none() {
        return Err(GraphError::InvalidRecord);
    }
    Ok(CalendarEventUpdateRequest {
        subject,
        start,
        end,
    })
}

fn calendar_event_update_times(
    update: &CalendarEventUpdate,
    source: &CalendarOccurrence,
) -> Result<
    (
        Option<CreateEventDateTimeTimeZone>,
        Option<CreateEventDateTimeTimeZone>,
    ),
    GraphError,
> {
    if source.time() == update.draft().time() {
        return Ok((None, None));
    }
    let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
        return Err(GraphError::InvalidRecord);
    };
    Ok((
        Some(CreateEventDateTimeTimeZone {
            date_time: graph_utc_datetime(*start)?,
            time_zone: "UTC",
        }),
        Some(CreateEventDateTimeTimeZone {
            date_time: graph_utc_datetime(*end)?,
            time_zone: "UTC",
        }),
    ))
}

fn confirm_calendar_event_update(
    update: &CalendarEventUpdate,
    current: &EditableCalendarEvent,
    response: &[u8],
) -> Result<CalendarOccurrence, GraphError> {
    let event_id = update.source().occurrence().id();
    let patch = calendar_event_update_patch(response, event_id)?;
    let mut event = current.occurrence().clone();
    patch
        .apply_to(&mut event)
        .map_err(|_| GraphError::CalendarEventUpdateOutcomeUnknown)?;
    if update.confirms(&event) {
        Ok(event)
    } else {
        Err(GraphError::CalendarEventUpdateOutcomeUnknown)
    }
}

fn calendar_event_update_patch(
    response: &[u8],
    expected_event_id: &EventId,
) -> Result<CalendarOccurrencePatch, GraphError> {
    let patch = serde_json::from_slice::<WireCalendarEvent>(response)
        .map_err(|_| GraphError::CalendarEventUpdateOutcomeUnknown)?
        .into_change()
        .map_err(|_| GraphError::CalendarEventUpdateOutcomeUnknown)?;
    let CalendarDeltaChange::Upsert { id, patch } = patch else {
        return Err(GraphError::CalendarEventUpdateOutcomeUnknown);
    };
    if &id != expected_event_id {
        return Err(GraphError::CalendarEventUpdateOutcomeUnknown);
    }
    Ok(patch)
}

pub(super) fn create_calendar_event_body(
    creation: &CalendarEventCreation,
) -> Result<Vec<u8>, GraphError> {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        return Err(GraphError::InvalidRecord);
    };
    let request = CreateCalendarEventRequest {
        subject: creation.draft().subject(),
        start: CreateEventDateTimeTimeZone {
            date_time: graph_utc_datetime(*start)?,
            time_zone: "UTC",
        },
        end: CreateEventDateTimeTimeZone {
            date_time: graph_utc_datetime(*end)?,
            time_zone: "UTC",
        },
        transaction_id: creation.id().as_uuid().to_string(),
    };
    serde_json::to_vec(&request).map_err(|_| GraphError::InvalidRecord)
}

fn graph_utc_datetime(value: OffsetDateTime) -> Result<String, GraphError> {
    if value.offset() != UtcOffset::UTC {
        return Err(GraphError::InvalidRecord);
    }
    value
        .format(&Rfc3339)
        .map_err(|_| GraphError::InvalidRecord)?
        .strip_suffix('Z')
        .map(str::to_owned)
        .ok_or(GraphError::InvalidRecord)
}

fn apply_calendar_content_fields(
    subject: WireField<String>,
    location: WireField<WireLocation>,
    patch: &mut CalendarOccurrencePatch,
) {
    match subject {
        WireField::Missing => {}
        WireField::Null => patch.set_subject(None),
        WireField::Value(subject) => patch.set_subject(Some(subject)),
    }
    match location {
        WireField::Missing => {}
        WireField::Null => patch.set_location(None),
        WireField::Value(location) => patch.set_location(location.display_name),
    }
}

fn apply_calendar_zone_fields(
    original_start_time_zone: WireField<String>,
    original_end_time_zone: WireField<String>,
    patch: &mut CalendarOccurrencePatch,
) {
    match original_start_time_zone {
        WireField::Missing => {}
        WireField::Null => patch.set_original_start_time_zone(None),
        WireField::Value(time_zone) => {
            patch.set_original_start_time_zone(Some(time_zone));
        }
    }
    match original_end_time_zone {
        WireField::Missing => {}
        WireField::Null => patch.set_original_end_time_zone(None),
        WireField::Value(time_zone) => {
            patch.set_original_end_time_zone(Some(time_zone));
        }
    }
}

fn calendar_occurrence_time_patch(
    start: WireField<WireDateTimeTimeZone>,
    end: WireField<WireDateTimeTimeZone>,
    is_all_day: WireField<bool>,
) -> Result<Option<CalendarOccurrenceTimePatch>, GraphError> {
    let mut time_patch = CalendarOccurrenceTimePatch::new();
    let start = parse_optional_calendar_time(start)?;
    let end = parse_optional_calendar_time(end)?;
    if let Some(start) = start {
        time_patch.set_start(start);
    }
    if let Some(end) = end {
        time_patch.set_end(end);
    }
    let has_all_day = apply_calendar_all_day_field(is_all_day, &mut time_patch)?;
    Ok((start.is_some() || end.is_some() || has_all_day).then_some(time_patch))
}

fn apply_calendar_all_day_field(
    field: WireField<bool>,
    time_patch: &mut CalendarOccurrenceTimePatch,
) -> Result<bool, GraphError> {
    match field {
        WireField::Missing => Ok(false),
        WireField::Null => Err(GraphError::InvalidRecord),
        WireField::Value(is_all_day) => {
            time_patch.set_is_all_day(is_all_day);
            Ok(true)
        }
    }
}

fn parse_optional_calendar_time(
    field: WireField<WireDateTimeTimeZone>,
) -> Result<Option<OffsetDateTime>, GraphError> {
    match field {
        WireField::Missing => Ok(None),
        WireField::Null => Err(GraphError::InvalidRecord),
        WireField::Value(date_time) => parse_graph_utc_date_time(&date_time).map(Some),
    }
}

fn parse_graph_utc_date_time(
    date_time: &WireDateTimeTimeZone,
) -> Result<OffsetDateTime, GraphError> {
    if date_time.time_zone != "UTC" {
        return Err(GraphError::InvalidRecord);
    }
    let date_time_utc = format!("{}Z", date_time.date_time);
    OffsetDateTime::parse(&date_time_utc, &Rfc3339).map_err(|_| GraphError::InvalidRecord)
}
