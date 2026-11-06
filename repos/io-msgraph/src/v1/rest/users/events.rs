//! Microsoft Graph events (`users.events`): list, get, create, update,
//! delete, the instances of a series, the calendar view and its delta.
//!
//! A recurring series is one `seriesMaster` event carrying the
//! recurrence; its `occurrence`s are computed, and an occurrence edited on
//! its own becomes an `exception`, both pointing at the master through
//! `seriesMasterId`. The events listing returns masters and lone events,
//! the calendar view and the instances listing expand occurrences.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/event>

use alloc::{string::String, vec::Vec};

use serde::{Deserialize, Serialize};

use crate::v1::{
    field::MsgraphField,
    rest::users::{
        contacts::{MsgraphPhysicalAddress, MsgraphSingleValueExtendedProperty},
        messages::{MsgraphEmailAddress, MsgraphImportance, MsgraphItemBody, MsgraphRecipient},
    },
};

pub mod calendar_view;
pub mod create;
pub mod delete;
pub mod delta;
pub mod get;
#[cfg(feature = "ical")]
pub mod ical;
pub mod instances;
pub mod list;
pub mod update;

/// An event of a calendar. Doubles as the create/update body.
///
/// Unset fields are left out (an update preserves them), null fields are
/// serialized as explicit nulls (an update clears them), and a set empty
/// collection clears the collection.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphEvent {
    /// The unique identifier of the event.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The iCalendar UID shared by every event of a series across
    /// calendars, read-only.
    #[serde(default, rename = "iCalUId", skip_serializing_if = "Option::is_none")]
    pub ical_uid: Option<String>,
    /// The version of the event, changing with every edit, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_key: Option<String>,
    /// When the event was created, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_date_time: Option<String>,
    /// When the event was last modified, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_date_time: Option<String>,
    /// The kind of event: lone, series master, occurrence or exception,
    /// read-only.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub event_type: Option<MsgraphEventType>,
    /// The series master of an occurrence or exception, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_master_id: Option<String>,
    /// The start the occurrence had in its series, for an occurrence or
    /// an exception, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_start: Option<String>,
    /// The zone the event's start was created in, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_start_time_zone: Option<String>,
    /// The zone the event's end was created in, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_end_time_zone: Option<String>,
    /// The subject.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub subject: MsgraphField<String>,
    /// The body.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub body: MsgraphField<MsgraphItemBody>,
    /// The start, a wall time in a zone.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub start: MsgraphField<MsgraphDateTimeTimeZone>,
    /// The end, a wall time in a zone.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub end: MsgraphField<MsgraphDateTimeTimeZone>,
    /// Whether the event lasts whole days, its start and end then being
    /// midnights.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub is_all_day: MsgraphField<bool>,
    /// The location.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub location: MsgraphField<MsgraphLocation>,
    /// The recurrence of a series master.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub recurrence: MsgraphField<MsgraphPatternedRecurrence>,
    /// The attendees.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub attendees: MsgraphField<Vec<MsgraphAttendee>>,
    /// The organizer, read-only once the event exists.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub organizer: MsgraphField<MsgraphRecipient>,
    /// The categories.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub categories: MsgraphField<Vec<String>>,
    /// How the event shows in the user's free/busy time.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub show_as: MsgraphField<MsgraphFreeBusyStatus>,
    /// The sensitivity.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub sensitivity: MsgraphField<MsgraphSensitivity>,
    /// The importance.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub importance: MsgraphField<MsgraphImportance>,
    /// Whether a reminder is set.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub is_reminder_on: MsgraphField<bool>,
    /// How long before the start the reminder fires, in minutes.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub reminder_minutes_before_start: MsgraphField<u32>,
    /// Whether the event was cancelled, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_cancelled: Option<bool>,
    /// Whether the user organizes the event, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_organizer: Option<bool>,
    /// The user's own response, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_status: Option<MsgraphResponseStatus>,
    /// The Outlook web link, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_link: Option<String>,
    /// Whether the event is an online meeting. Setting it makes Graph
    /// create one with the calendar's default provider (Teams), whose
    /// details then come back in `onlineMeeting`.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub is_online_meeting: MsgraphField<bool>,
    /// The details to join an online meeting (Teams), read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_meeting: Option<MsgraphOnlineMeetingInfo>,
    /// The occurrence ids (`OID.{seriesMasterId}.{date}`) of a series'
    /// cancelled occurrences, on a series master, read-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cancelled_occurrences: Vec<String>,
    /// Single-value extended properties, returned only when expanded.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub single_value_extended_properties: MsgraphField<Vec<MsgraphSingleValueExtendedProperty>>,
}

/// A wall time in a named zone, Graph's time representation.
///
/// The zone is a Windows name (`Romance Standard Time`) or an IANA one,
/// depending on how the event was created, and `UTC` for a UTC time.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphDateTimeTimeZone {
    /// The wall time, `YYYY-MM-DDTHH:MM:SS` with optional fractional
    /// seconds.
    pub date_time: String,
    /// The zone the wall time is read in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
}

/// The details to join an online meeting, read-only.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphOnlineMeetingInfo {
    /// The link to join the meeting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_url: Option<String>,
}

/// An event location.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphLocation {
    /// The name shown for the location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// A URI for the location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_uri: Option<String>,
    /// The street address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<MsgraphPhysicalAddress>,
}

/// The recurrence of a series: a pattern and the range it repeats over.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphPatternedRecurrence {
    /// How often the event repeats.
    pub pattern: MsgraphRecurrencePattern,
    /// When the repetition starts and stops.
    pub range: MsgraphRecurrenceRange,
}

/// How often a series repeats.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphRecurrencePattern {
    /// The kind of pattern.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub pattern_type: Option<MsgraphRecurrencePatternType>,
    /// The number of units between occurrences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<u32>,
    /// The month of a yearly pattern, 1 to 12.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u32>,
    /// The day of an absolute monthly or yearly pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_of_month: Option<u32>,
    /// The weekdays of a weekly or relative pattern.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days_of_week: Vec<MsgraphDayOfWeek>,
    /// The first day of the week of a weekly pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_day_of_week: Option<MsgraphDayOfWeek>,
    /// Which week of the month a relative pattern names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<MsgraphWeekIndex>,
}

/// When a series starts and stops repeating.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphRecurrenceRange {
    /// How the range ends.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub range_type: Option<MsgraphRecurrenceRangeType>,
    /// The first date of the series, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    /// The last date of an `endDate` range, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    /// The zone the dates are read in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_time_zone: Option<String>,
    /// The number of occurrences of a `numbered` range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_of_occurrences: Option<u32>,
}

/// An event attendee.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphAttendee {
    /// Whether the attendee is required, optional or a resource.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub attendee_type: Option<MsgraphAttendeeType>,
    /// The attendee's response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<MsgraphResponseStatus>,
    /// The attendee's name and address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_address: Option<MsgraphEmailAddress>,
}

/// A response to an invitation.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphResponseStatus {
    /// The response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<MsgraphResponseType>,
    /// When it was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
}

/// The kind of an event within a series.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphEventType {
    /// A lone event.
    SingleInstance,
    /// An occurrence of a series, computed from its recurrence.
    Occurrence,
    /// An occurrence edited on its own.
    Exception,
    /// The event carrying a series' recurrence.
    SeriesMaster,
}

/// How an event shows in free/busy time.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphFreeBusyStatus {
    /// Unknown status.
    Unknown,
    /// Free.
    Free,
    /// Tentative.
    Tentative,
    /// Busy.
    Busy,
    /// Out of office.
    Oof,
    /// Working elsewhere.
    WorkingElsewhere,
}

/// How private an event is.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphSensitivity {
    /// Normal.
    Normal,
    /// Personal.
    Personal,
    /// Private.
    Private,
    /// Confidential.
    Confidential,
}

/// A response to an invitation.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphResponseType {
    /// No response.
    None,
    /// The organizer, who does not respond.
    Organizer,
    /// Tentatively accepted.
    TentativelyAccepted,
    /// Accepted.
    Accepted,
    /// Declined.
    Declined,
    /// Not responded yet.
    NotResponded,
}

/// The role of an attendee.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphAttendeeType {
    /// A required attendee.
    Required,
    /// An optional attendee.
    Optional,
    /// A resource, a room or equipment.
    Resource,
}

/// The kind of a recurrence pattern.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphRecurrencePatternType {
    /// Every `interval` days.
    Daily,
    /// On `daysOfWeek`, every `interval` weeks.
    Weekly,
    /// On `dayOfMonth`, every `interval` months.
    AbsoluteMonthly,
    /// On the `index` weekday of `daysOfWeek`, every `interval` months.
    RelativeMonthly,
    /// On `dayOfMonth` of `month`, every `interval` years.
    AbsoluteYearly,
    /// On the `index` weekday of `daysOfWeek` in `month`, every
    /// `interval` years.
    RelativeYearly,
}

/// How a recurrence range ends.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphRecurrenceRangeType {
    /// On `endDate`.
    EndDate,
    /// Never.
    NoEnd,
    /// After `numberOfOccurrences`.
    Numbered,
}

/// A day of the week.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphDayOfWeek {
    /// Sunday.
    Sunday,
    /// Monday.
    Monday,
    /// Tuesday.
    Tuesday,
    /// Wednesday.
    Wednesday,
    /// Thursday.
    Thursday,
    /// Friday.
    Friday,
    /// Saturday.
    Saturday,
}

/// Which week of the month a relative pattern names.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphWeekIndex {
    /// The first.
    First,
    /// The second.
    Second,
    /// The third.
    Third,
    /// The fourth.
    Fourth,
    /// The last.
    Last,
}
