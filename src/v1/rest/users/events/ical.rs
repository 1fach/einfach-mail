//! # iCalendar projection
//!
//! Projects a Microsoft Graph event onto iCalendar and back. Graph exposes
//! no iCalendar form of an event, so a consumer needing one (a calendar
//! client, a sync engine) synthesizes the document of record from the JSON
//! resource ([`MsgraphEvent::to_ical`], [`MsgraphEvent::to_ical_series`])
//! and reads it back ([`MsgraphEvent::from_ical`]).
//!
//! A field is managed only where it has a well-defined iCalendar slot:
//!
//! - the recurrence pattern maps onto an RRULE and back, a rule Graph
//!   cannot hold being refused rather than approximated; the cancelled
//!   occurrences of a series become EXDATEs, and an exception, an
//!   occurrence edited on its own, its own VEVENT with a RECURRENCE-ID;
//! - Graph names zones the Windows way when Outlook created the event, so
//!   a Windows name is read as its CLDR IANA counterpart, every TZID gets a
//!   VTIMEZONE from the bundled database, and a zone neither table knows
//!   keeps its name, undefined;
//! - `showAs` rides the `X-MICROSOFT-CDO-BUSYSTATUS` property Outlook
//!   itself writes; the web link is minted read-only as
//!   `X-MSGRAPH-WEB-LINK`, and the join link of an online meeting as a
//!   standard `CONFERENCE` (RFC 7986 5.11);
//! - `X-PIMDIR-ONLINE-MEETING:TRUE` (pimdir STORAGE Annex B.1) asks for
//!   an online meeting: it sets `isOnlineMeeting`, Graph creating the
//!   meeting with the calendar's default provider, and is never written
//!   back;
//! - an invitation that came in by mail has Exchange's global object id
//!   for `iCalUId`; when it wraps the organizer's UID (the `vCal-Uid`
//!   form), that UID is the one projected ([`original_uid`]);
//! - everything else, the UID among it, Graph minting its own `iCalUId`, is
//!   stashed verbatim in a single-value extended property
//!   ([`MSGRAPH_EVENT_STASH_ID`]) and spliced back on read; Graph returns it
//!   only expanded ([`MSGRAPH_EVENT_STASH_EXPAND`]).
//!
//! A write of a document goes to the series master alone: an exception
//! edited locally, a cancelled occurrence and an EXDATE do not push with
//! it, Graph taking them only through the occurrence itself, an instance
//! of the series. [`MsgraphEvent::instance_update_from_ical`] projects one
//! occurrence onto the update of its instance, and a consumer deletes the
//! instance of an occurrence the document cancels.

mod windows_zones;

use core::fmt;

use alloc::{
    borrow::{Cow, ToOwned},
    collections::BTreeSet,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use ical::{
    param::IcalParam,
    prop::{IcalProp, IcalPropKind, IcalPropName, action::ACTION, trigger::TRIGGER},
    tree::{
        codec::Codec,
        cst::{IcalCst, IcalItem},
        error::IcalParseError,
        line::IcalLine,
        param::{
            cn::CN, cutype::CUTYPE, fmttype::FMTTYPE, partstat::PARTSTAT, role::ROLE, tzid::TZID,
            value::VALUE,
        },
    },
    tzdb,
    value::{
        IcalValue,
        datetime::IcalDateTime,
        recur::IcalRecur,
        text::{IcalText, IcalTextList},
    },
};
use jiff::{
    Span, Timestamp,
    civil::{Date, DateTime},
    tz::TimeZone,
};
use serde_json::{from_value, to_value};

use crate::v1::{
    field::MsgraphField,
    rest::users::{
        contacts::MsgraphSingleValueExtendedProperty,
        events::{
            MsgraphAttendee, MsgraphAttendeeType, MsgraphDateTimeTimeZone, MsgraphDayOfWeek,
            MsgraphEvent, MsgraphFreeBusyStatus, MsgraphLocation, MsgraphPatternedRecurrence,
            MsgraphRecurrencePattern, MsgraphRecurrencePatternType, MsgraphRecurrenceRange,
            MsgraphRecurrenceRangeType, MsgraphResponseStatus, MsgraphResponseType,
            MsgraphSensitivity, MsgraphWeekIndex,
        },
        messages::{MsgraphBodyType, MsgraphEmailAddress, MsgraphImportance, MsgraphItemBody},
    },
};

/// Extended-property id under which the iCalendar remainder of an event is
/// stashed, a fixed app GUID plus a name (the String MAPI form).
pub const MSGRAPH_EVENT_STASH_ID: &str =
    "String {c8e5e5cf-3f6c-4f0a-9d4e-52f1e7b2a9d3} Name pimalaya-ical";

/// The `$expand` clause reading the stash back with an event, which Graph
/// omits otherwise.
pub const MSGRAPH_EVENT_STASH_EXPAND: &str = "singleValueExtendedProperties($filter=id eq \
     'String {c8e5e5cf-3f6c-4f0a-9d4e-52f1e7b2a9d3} Name pimalaya-ical')";

/// The `$select` of a read the projection is fed from: every property it
/// reads, `cancelledOccurrences` among them, which Graph returns only
/// selected, and the zones an event was created in, which give back the
/// wall time Graph answers in UTC.
///
/// Graph returns `cancelledOccurrences` only on a read of a series master
/// by its id, never in a listing: project a master read with
/// `event_get`, else its cancelled occurrences get no EXDATE.
pub const MSGRAPH_EVENT_ICAL_SELECT: &str = "id,iCalUId,changeKey,createdDateTime,\
     lastModifiedDateTime,type,seriesMasterId,originalStart,originalStartTimeZone,\
     originalEndTimeZone,subject,body,start,end,isAllDay,location,recurrence,attendees,\
     organizer,categories,showAs,sensitivity,importance,isReminderOn,\
     reminderMinutesBeforeStart,isCancelled,webLink,onlineMeeting,cancelledOccurrences";

/// Product identifier the synthesized document carries.
pub(crate) const PRODID: &str = "-//Pimalaya//io-msgraph//EN";

/// Longest raw property line stashed server-side; a longer one, an inline
/// attachment essentially, stays in the local document.
pub(crate) const MAX_STASH_LINE: usize = 8 * 1024;

/// Properties minted read-only from Graph-scoped fields, dropped on read.
const MINTED_PROPS: &[&str] = &["X-MSGRAPH-WEB-LINK", "CONFERENCE", TEAMS_MEETING_URL];

/// The Outlook property the join link of an online meeting was minted as
/// before CONFERENCE, still dropped from the documents a store kept since.
const TEAMS_MEETING_URL: &str = "X-MICROSOFT-SKYPETEAMSMEETINGURL";

/// The property asking the source for an online meeting of its provider
/// (pimdir STORAGE Annex B.1).
const ONLINE_MEETING: &str = "X-PIMDIR-ONLINE-MEETING";

/// The Outlook busy status extension, carrying `showAs` both ways.
const BUSY_STATUS: &str = "X-MICROSOFT-CDO-BUSYSTATUS";

/// The HTML alternative description Outlook and Thunderbird exchange.
const ALT_DESC: &str = "X-ALT-DESC";

impl MsgraphEvent {
    /// Projects a lone event or a series master onto a fresh VCALENDAR
    /// document.
    pub fn to_ical(&self) -> String {
        self.to_ical_series(&[])
    }

    /// Projects a series master and its exceptions onto one VCALENDAR
    /// document, the exceptions as VEVENTs carrying the master's UID and a
    /// RECURRENCE-ID.
    ///
    /// Read the exceptions with [`MSGRAPH_EVENT_ICAL_SELECT`] too: the
    /// default instance listing leaves out `originalStart`, without which
    /// an exception gets no RECURRENCE-ID and reads as a second master.
    pub fn to_ical_series(&self, exceptions: &[&MsgraphEvent]) -> String {
        let master = localized(self);
        let master = &*master;
        let exceptions: Vec<Cow<'_, MsgraphEvent>> = exceptions
            .iter()
            .map(|exception| localized(exception))
            .collect();
        let uid = master
            .stashed_uid()
            .or_else(|| master.ical_uid.as_deref().map(original_uid))
            .unwrap_or_else(|| master.id.clone());

        let mut zones = BTreeSet::new();
        let mut events = vec![vevent(master, None, &uid, &mut zones)];
        for exception in &exceptions {
            events.push(vevent(exception, Some(master), &uid, &mut zones));
        }

        let mut calendar = IcalCst::v2();
        calendar.push(IcalProp::text(IcalPropKind::ProdId, vec![], PRODID));

        let anchor = anchor(master);
        for zone in &zones {
            if let Some(vtimezone) = tzdb::vtimezone(zone, anchor) {
                calendar.push_component(vtimezone);
            }
        }
        for event in events {
            calendar.push_component(event);
        }

        String::from_utf8_lossy(&calendar.to_bytes()).into_owned()
    }

    /// Projects an iCalendar document onto an event, in full state: each
    /// managed field is Set from the series master or Null when absent,
    /// while Graph-only fields stay Unset.
    ///
    /// The exceptions of a series are not read: Graph takes each through
    /// its instance ([`Self::instance_update_from_ical`]).
    pub fn from_ical(contents: &[u8]) -> Result<Self, MsgraphEventIcalError> {
        let calendar = IcalCst::parse(contents).map_err(MsgraphEventIcalError::Parse)?;
        project(find_vevent(&calendar, |vevent| {
            !has_prop(vevent, "RECURRENCE-ID")
        })?)
    }

    /// Projects an iCalendar document onto a create body: the full-state
    /// projection minus its nulls and empty collections, which a fresh event
    /// does not need.
    pub fn create_from_ical(contents: &[u8]) -> Result<Self, MsgraphEventIcalError> {
        let mut event = Self::from_ical(contents)?;

        if stash_lines(&event.single_value_extended_properties).is_empty() {
            event.single_value_extended_properties = MsgraphField::Unset;
        }

        let mut body = to_value(&event).map_err(MsgraphEventIcalError::Json)?;
        if let Some(object) = body.as_object_mut() {
            object.retain(|_, value| {
                !value.is_null() && value.as_array().is_none_or(|array| !array.is_empty())
            });
        }

        from_value(body).map_err(MsgraphEventIcalError::Json)
    }

    /// Projects an iCalendar document onto an update body: the fields that
    /// differ from the base document (the state last synced), the unchanged
    /// ones left Unset and out of the PATCH.
    pub fn update_from_ical(contents: &[u8], base: &[u8]) -> Result<Self, MsgraphEventIcalError> {
        let mut event = Self::from_ical(contents)?;
        event.unset_unchanged(&Self::from_ical(base)?);
        Ok(event)
    }

    /// Projects one occurrence of a series onto the update body of its
    /// Graph instance: the fields it sets differently from `base`, the
    /// occurrence as Graph holds it; `None` when the two agree.
    ///
    /// Each document holds that occurrence alone, as its first VEVENT: an
    /// override with its RECURRENCE-ID, or the series' own component
    /// carried to the occurrence, which is what reverts an exception.
    /// Neither the recurrence nor the stash is sent, Graph keeping both on
    /// the series master.
    pub fn instance_update_from_ical(
        contents: &[u8],
        base: &[u8],
    ) -> Result<Option<Self>, MsgraphEventIcalError> {
        let occurrence = |contents| {
            let calendar = IcalCst::parse(contents).map_err(MsgraphEventIcalError::Parse)?;
            project(find_vevent(&calendar, |_| true)?)
        };

        let mut event = occurrence(contents)?;
        event.unset_unchanged(&occurrence(base)?);
        event.recurrence = MsgraphField::Unset;
        event.single_value_extended_properties = MsgraphField::Unset;

        Ok((event != Self::default()).then_some(event))
    }

    /// Leaves Unset every managed field `base` holds the same.
    fn unset_unchanged(&mut self, base: &Self) {
        macro_rules! unset_unchanged {
            ($($field:ident),* $(,)?) => {$(
                if self.$field == base.$field {
                    self.$field = MsgraphField::Unset;
                }
            )*};
        }

        if same_text(&body_text(&self.body), &body_text(&base.body)) {
            self.body = MsgraphField::Unset;
        }

        unset_unchanged!(
            subject,
            start,
            end,
            is_all_day,
            location,
            recurrence,
            attendees,
            categories,
            show_as,
            sensitivity,
            importance,
            is_reminder_on,
            reminder_minutes_before_start,
            is_online_meeting,
            single_value_extended_properties,
        );
    }

    /// The RECURRENCE-ID value the projection gives the occurrence of this
    /// series master that originally started at `original_start` (an
    /// instance's `originalStart`): a date for an all-day series, the wall
    /// time in the series' zone, or a UTC stamp when that zone is unknown.
    ///
    /// A consumer told an occurrence by its RECURRENCE-ID finds the Graph
    /// instance with it, comparing values the way the projection wrote them.
    pub fn recurrence_id_of(&self, original_start: &str) -> Option<String> {
        let master = localized(self);
        let prop = recurrence_id(&master, original_start, &mut BTreeSet::new())?;
        match prop.value {
            IcalValue::Date(date) => Some(date.0.into_owned()),
            IcalValue::DateTime(stamp) => Some(stamp.0.into_owned()),
            _ => None,
        }
    }

    /// The iCalendar UID the stash carries, `None` for an event no document
    /// was ever written to, Graph minting its own `iCalUId`.
    ///
    /// A sync engine checks it after a write: a server that dropped the
    /// stash would hand the event back under Graph's UID rather than the one
    /// it was written with.
    pub fn stashed_uid(&self) -> Option<String> {
        stash_lines(&self.single_value_extended_properties)
            .iter()
            .find_map(|line| {
                let mut bytes = line.clone();
                bytes.push_str("\r\n");
                let (line, _) = IcalLine::take(bytes.as_bytes()).ok()?;
                line.name
                    .get()
                    .eq_ignore_ascii_case("UID")
                    .then(|| line.raw_value_str().trim().to_string())
                    .filter(|uid| !uid.is_empty())
            })
    }
}

impl MsgraphPatternedRecurrence {
    /// The first and last dates a series spans, inclusive, the last
    /// `None` for a `noEnd` range.
    ///
    /// The last date follows the range type: the `endDate` of an `endDate`
    /// range, a bound past the last occurrence of a `numbered` one, sized
    /// `numberOfOccurrences` times `interval` periods of the pattern. Graph
    /// fills the `endDate` of the other types with `0001-01-01`, which is
    /// no end.
    pub fn bounds(&self) -> Option<(Date, Option<Date>)> {
        let range = &self.range;
        let start = range.start_date.as_deref()?.parse::<Date>().ok()?;

        let end = match range.range_type? {
            MsgraphRecurrenceRangeType::EndDate => {
                Some(range.end_date.as_deref()?.parse::<Date>().ok()?)
            }
            MsgraphRecurrenceRangeType::Numbered => {
                let periods = i64::from(range.number_of_occurrences?)
                    * i64::from(self.pattern.interval.unwrap_or(1).max(1));
                let span = match self.pattern.pattern_type? {
                    MsgraphRecurrencePatternType::Daily => Span::new().try_days(periods),
                    MsgraphRecurrencePatternType::Weekly => Span::new().try_weeks(periods),
                    MsgraphRecurrencePatternType::AbsoluteMonthly
                    | MsgraphRecurrencePatternType::RelativeMonthly => {
                        Span::new().try_months(periods)
                    }
                    MsgraphRecurrencePatternType::AbsoluteYearly
                    | MsgraphRecurrencePatternType::RelativeYearly => {
                        Span::new().try_years(periods)
                    }
                };
                Some(start.checked_add(span.ok()?).ok()?)
            }
            MsgraphRecurrenceRangeType::NoEnd => None,
        };

        Some((start, end))
    }
}

/// An iCalendar document that cannot become a Graph event.
#[derive(Debug)]
pub enum MsgraphEventIcalError {
    /// The document does not parse.
    Parse(IcalParseError),
    /// The document holds no VEVENT.
    NoEvent,
    /// The document holds a component Graph does not model.
    NotAnEvent(String),
    /// The VEVENT starts neither on a date, in UTC nor in a named zone.
    NoZonedStart,
    /// The VEVENT has neither a DTEND nor a DURATION.
    NoEnd,
    /// The RRULE has no Graph recurrence pattern.
    UnsupportedRecurrence(String),
    /// The projected event does not reshape through JSON.
    Json(serde_json::Error),
}

impl fmt::Display for MsgraphEventIcalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "Invalid iCalendar: {err}"),
            Self::NoEvent => write!(f, "The iCalendar contents carry no VEVENT"),
            Self::NotAnEvent(name) => {
                write!(
                    f,
                    "Microsoft Graph models no {name}: its calendars hold events only"
                )
            }
            Self::NoZonedStart => write!(
                f,
                "Microsoft Graph needs a DTSTART that is a date, in UTC, or in a named zone"
            ),
            Self::NoEnd => write!(f, "The VEVENT has neither a DTEND nor a DURATION"),
            Self::UnsupportedRecurrence(rule) => {
                write!(f, "Microsoft Graph cannot hold the recurrence `{rule}`")
            }
            Self::Json(err) => write!(f, "Cannot shape the Graph event body: {err}"),
        }
    }
}

impl core::error::Error for MsgraphEventIcalError {}

/// Projects one VEVENT onto an event, in full state.
fn project(vevent: &IcalCst<'_>) -> Result<MsgraphEvent, MsgraphEventIcalError> {
    let mut event = MsgraphEvent {
        subject: MsgraphField::Null,
        body: MsgraphField::Null,
        location: MsgraphField::Null,
        recurrence: MsgraphField::Null,
        attendees: MsgraphField::Set(Vec::new()),
        categories: MsgraphField::Set(Vec::new()),
        show_as: MsgraphField::Null,
        sensitivity: MsgraphField::Null,
        importance: MsgraphField::Null,
        is_reminder_on: MsgraphField::Set(false),
        ..Default::default()
    };

    let mut description = None;
    let mut html = None;
    let mut start = None;
    let mut end = None;
    let mut duration = None;
    let mut transparent = None;
    let mut rrule = None;
    let mut stash = Vec::new();

    for item in &vevent.items {
        match item {
            IcalItem::Prop(line) => {
                let name = line.name.get();
                let consumed = if MINTED_PROPS.iter().any(|m| name.eq_ignore_ascii_case(m)) {
                    true
                } else if name.eq_ignore_ascii_case(ONLINE_MEETING) {
                    if line.raw_value_str().trim().eq_ignore_ascii_case("TRUE") {
                        event.is_online_meeting = MsgraphField::Set(true);
                    }
                    true
                } else if name.eq_ignore_ascii_case(BUSY_STATUS) {
                    event.show_as =
                        MsgraphField::set_or_null(busy_status_from_ical(&line.raw_value_str()));
                    true
                } else if name.eq_ignore_ascii_case(ALT_DESC)
                    && line
                        .param::<FMTTYPE>()
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("text/html"))
                {
                    html = Some(text(line));
                    true
                } else {
                    match name.parse::<IcalPropKind>() {
                        Ok(IcalPropKind::Summary) => {
                            event.subject = MsgraphField::Set(text(line));
                            true
                        }
                        Ok(IcalPropKind::Description) => {
                            description = Some(text(line));
                            true
                        }
                        Ok(IcalPropKind::Location) => {
                            event.location = MsgraphField::Set(MsgraphLocation {
                                display_name: Some(text(line)),
                                ..Default::default()
                            });
                            true
                        }
                        Ok(IcalPropKind::DtStart) => {
                            start = Some(boundary(line));
                            true
                        }
                        Ok(IcalPropKind::DtEnd) => {
                            end = Some(boundary(line));
                            true
                        }
                        Ok(IcalPropKind::Duration) => {
                            duration = line.raw_value_str().trim().parse::<Span>().ok();
                            duration.is_some()
                        }
                        Ok(IcalPropKind::RRule) => {
                            rrule = Some(IcalRecur::decode(&line.value).0.trim().to_owned());
                            true
                        }
                        // NOTE: Graph mints the EXDATEs from the cancelled
                        // occurrences, which only a deletion of the
                        // instance writes; a stashed copy would outlive it.
                        Ok(IcalPropKind::ExDate) => true,
                        Ok(IcalPropKind::Categories) => {
                            let list = IcalTextList::decode(&line.value);
                            if let MsgraphField::Set(categories) = &mut event.categories {
                                categories.extend(list.0.into_iter().map(Cow::into_owned));
                            }
                            true
                        }
                        Ok(IcalPropKind::Class) => {
                            event.sensitivity = MsgraphField::set_or_null(sensitivity_from_ical(
                                &line.raw_value_str(),
                            ));
                            true
                        }
                        Ok(IcalPropKind::Priority) => {
                            event.importance = MsgraphField::set_or_null(importance_from_ical(
                                &line.raw_value_str(),
                            ));
                            true
                        }
                        Ok(IcalPropKind::Transp) => {
                            transparent = Some(
                                line.raw_value_str()
                                    .trim()
                                    .eq_ignore_ascii_case("TRANSPARENT"),
                            );
                            true
                        }
                        Ok(IcalPropKind::Attendee) => {
                            if let MsgraphField::Set(attendees) = &mut event.attendees {
                                attendees.push(attendee(line));
                            }
                            true
                        }
                        // NOTE: Graph sets the organizer, the status and
                        // the stamps itself, so an incoming value is
                        // consumed rather than stashed.
                        Ok(
                            IcalPropKind::Organizer
                            | IcalPropKind::Status
                            | IcalPropKind::DtStamp
                            | IcalPropKind::Created
                            | IcalPropKind::LastModified
                            | IcalPropKind::Sequence,
                        ) => true,
                        _ => false,
                    }
                };

                if !consumed {
                    let raw = raw_line(line);
                    if raw.len() <= MAX_STASH_LINE {
                        stash.push(raw);
                    }
                }
            }
            IcalItem::Component(alarm) => {
                let reminder = reminder_minutes(alarm);
                match (&event.reminder_minutes_before_start, reminder) {
                    (MsgraphField::Unset, Some(minutes)) => {
                        event.is_reminder_on = MsgraphField::Set(true);
                        event.reminder_minutes_before_start = MsgraphField::Set(minutes);
                    }
                    _ => stash.extend(raw_component(alarm)),
                }
            }
            IcalItem::Opaque(bytes) => stash.push(String::from_utf8_lossy(bytes).into_owned()),
        }
    }

    let Some(Some(start)) = start else {
        return Err(MsgraphEventIcalError::NoZonedStart);
    };
    let end = match (end, duration) {
        (Some(Some(end)), _) => end,
        (_, Some(duration)) => start
            .shifted(duration)
            .ok_or(MsgraphEventIcalError::NoEnd)?,
        _ => return Err(MsgraphEventIcalError::NoEnd),
    };

    event.is_all_day = MsgraphField::Set(start.all_day);
    if let Some(rule) = &rrule {
        event.recurrence = MsgraphField::Set(recurrence(rule, &start)?);
    }
    event.start = MsgraphField::Set(start.into_graph());
    event.end = MsgraphField::Set(end.into_graph());

    event.body = match (html, description) {
        // NOTE: the text is what an editor changes, so an HTML copy
        // it no longer matches is stale and must not win.
        (Some(html), Some(content)) if !same_text(&strip_html(&html), &content) => {
            MsgraphField::Set(MsgraphItemBody {
                content_type: Some(MsgraphBodyType::Text),
                content: Some(content),
            })
        }
        (Some(content), _) => MsgraphField::Set(MsgraphItemBody {
            content_type: Some(MsgraphBodyType::Html),
            content: Some(content),
        }),
        (None, Some(content)) => MsgraphField::Set(MsgraphItemBody {
            content_type: Some(MsgraphBodyType::Text),
            content: Some(content),
        }),
        // NOTE: Graph refuses a null body, an empty text one is how
        // it clears.
        (None, None) => MsgraphField::Set(MsgraphItemBody {
            content_type: Some(MsgraphBodyType::Text),
            content: Some(String::new()),
        }),
    };

    if matches!(event.show_as, MsgraphField::Null) {
        event.show_as = match transparent {
            Some(true) => MsgraphField::Set(MsgraphFreeBusyStatus::Free),
            Some(false) => MsgraphField::Set(MsgraphFreeBusyStatus::Busy),
            None => MsgraphField::Null,
        };
    }

    // NOTE: the stash entry is always Set (empty when nothing is left),
    // so an update can tell a cleared stash from an unchanged one.
    event.single_value_extended_properties =
        MsgraphField::Set(vec![MsgraphSingleValueExtendedProperty {
            id: MSGRAPH_EVENT_STASH_ID.to_string(),
            value: stash.join("\n"),
        }]);

    Ok(event)
}

/// Builds one VEVENT; `master` is the series of an exception.
fn vevent(
    event: &MsgraphEvent,
    master: Option<&MsgraphEvent>,
    uid: &str,
    zones: &mut BTreeSet<String>,
) -> IcalCst<'static> {
    let mut vevent = IcalCst::empty("VEVENT");
    vevent.push(IcalProp::text(IcalPropKind::Uid, vec![], uid.to_owned()));

    // NOTE: DTSTAMP is mandatory (RFC 5545 3.6.1) and Graph keeps no field
    // of its own for it, so the last modification stands in.
    let stamp = event
        .last_modified_date_time
        .as_deref()
        .or(event.created_date_time.as_deref())
        .and_then(utc_stamp);
    if let Some(stamp) = &stamp {
        vevent.push(stamp_prop(IcalPropKind::DtStamp, stamp.clone()));
    }

    let all_day = event.is_all_day.as_option().copied().unwrap_or(false);
    if let Some(start) = event.start.as_option() {
        vevent.push(boundary_prop(IcalPropKind::DtStart, start, all_day, zones));
    }
    if let Some(end) = event.end.as_option() {
        vevent.push(boundary_prop(IcalPropKind::DtEnd, end, all_day, zones));
    }

    if let (Some(master), Some(original)) = (master, &event.original_start)
        && let Some(prop) = recurrence_id(master, original, zones)
    {
        vevent.push(prop);
    }

    if let Some(subject) = event.subject.as_option() {
        vevent.push(IcalProp::text(
            IcalPropKind::Summary,
            vec![],
            subject.clone(),
        ));
    }

    if let Some(body) = event.body.as_option() {
        let content = body.content.clone().unwrap_or_default();
        match body.content_type {
            Some(MsgraphBodyType::Html) => {
                vevent.push(IcalProp::text(
                    IcalPropKind::Description,
                    vec![],
                    strip_html(&content),
                ));
                vevent.push(IcalProp::text(
                    ALT_DESC,
                    vec![IcalParam::FmtType("text/html".into())],
                    content,
                ));
            }
            _ if !content.is_empty() => {
                vevent.push(IcalProp::text(IcalPropKind::Description, vec![], content));
            }
            _ => {}
        }
    }

    if let Some(name) = event
        .location
        .as_option()
        .and_then(|location| location.display_name.clone())
        .filter(|name| !name.is_empty())
    {
        vevent.push(IcalProp::text(IcalPropKind::Location, vec![], name));
    }

    if event.is_cancelled == Some(true) {
        vevent.push(IcalProp::text(IcalPropKind::Status, vec![], "CANCELLED"));
    }

    if let Some(show_as) = event.show_as.as_option() {
        let transp = match show_as {
            MsgraphFreeBusyStatus::Free => "TRANSPARENT",
            _ => "OPAQUE",
        };
        vevent.push(IcalProp::text(IcalPropKind::Transp, vec![], transp));
        vevent.push(IcalProp::text(
            BUSY_STATUS,
            vec![],
            busy_status_to_ical(*show_as),
        ));
    }

    if let Some(class) = event
        .sensitivity
        .as_option()
        .map(|s| sensitivity_to_ical(*s))
    {
        vevent.push(IcalProp::text(IcalPropKind::Class, vec![], class));
    }

    if let Some(importance) = event.importance.as_option() {
        let priority = match importance {
            MsgraphImportance::High => "1",
            MsgraphImportance::Normal => "5",
            MsgraphImportance::Low => "9",
        };
        vevent.push(IcalProp {
            name: IcalPropName::Kind(IcalPropKind::Priority),
            params: Vec::new(),
            value: IcalValue::Integer(priority.into()),
        });
    }

    let categories: Vec<Cow<'static, str>> = event
        .categories
        .as_option()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter(|category| !category.is_empty())
        .map(|category| Cow::Owned(category.clone()))
        .collect();
    if !categories.is_empty() {
        vevent.push(IcalProp {
            name: IcalPropName::Kind(IcalPropKind::Categories),
            params: Vec::new(),
            value: IcalValue::TextList(IcalTextList(categories)),
        });
    }

    for (kind, stamp) in [
        (IcalPropKind::Created, &event.created_date_time),
        (IcalPropKind::LastModified, &event.last_modified_date_time),
    ] {
        if let Some(stamp) = stamp.as_deref().and_then(utc_stamp) {
            vevent.push(stamp_prop(kind, stamp));
        }
    }

    if let Some(organizer) = event
        .organizer
        .as_option()
        .map(|organizer| &organizer.email_address)
    {
        vevent.push(person_prop(IcalPropKind::Organizer, organizer, Vec::new()));
    }

    for attendee in event
        .attendees
        .as_option()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        if let Some(prop) = attendee_prop(attendee) {
            vevent.push(prop);
        }
    }

    if master.is_none()
        && let Some(recurrence) = event.recurrence.as_option()
    {
        if let Some(rule) = rrule(recurrence, event.start.as_option(), all_day) {
            vevent.push(IcalProp {
                name: IcalPropName::Kind(IcalPropKind::RRule),
                params: Vec::new(),
                value: IcalValue::Recur(rule.into()),
            });
        }
        for exdate in exdates(event, all_day, zones) {
            vevent.push(exdate);
        }
    }

    if let Some(link) = &event.web_link {
        vevent.push(IcalProp::text("X-MSGRAPH-WEB-LINK", vec![], link.clone()));
    }

    if let Some(url) = event
        .online_meeting
        .as_ref()
        .and_then(|meeting| meeting.join_url.as_deref())
        .filter(|url| !url.is_empty())
    {
        vevent.push(IcalProp {
            name: IcalPropName::Kind(IcalPropKind::Conference),
            params: vec![
                IcalParam::Value("URI".into()),
                IcalParam::Feature(vec!["AUDIO".into(), "VIDEO".into()]),
            ],
            value: IcalValue::Uri(url.to_owned().into()),
        });
    }

    // NOTE: a minted property stashed by an older version would come out
    // twice.
    let stash: Vec<String> = stash_lines(&event.single_value_extended_properties)
        .into_iter()
        .filter(|line| !is_uid_line(line) && !is_minted_line(line))
        .collect();
    push_stash(&mut vevent, &stash);

    if event.is_reminder_on.as_option() == Some(&true)
        && let Some(minutes) = event.reminder_minutes_before_start.as_option()
    {
        let mut alarm = IcalCst::empty("VALARM");
        alarm.push(IcalProp::text(IcalPropKind::Action, vec![], "DISPLAY"));
        alarm.push(IcalProp {
            name: IcalPropName::Kind(IcalPropKind::Trigger),
            params: Vec::new(),
            value: IcalValue::Duration(format!("-PT{minutes}M").into()),
        });
        alarm.push(IcalProp::text(
            IcalPropKind::Description,
            vec![],
            "Reminder",
        ));
        vevent.push_component(alarm);
    }

    vevent
}

/// The event with a UTC start and end told back in the zones it was
/// created in.
///
/// Graph answers a read in UTC unless the request names a zone, and keeps
/// the creation zones as `originalStartTimeZone` and `originalEndTimeZone`.
/// Left in UTC, a series anchored on a wall time drifts by an hour at each
/// DST change once expanded. All-day boundaries are dates, and a zone the
/// database does not know stays UTC.
fn localized(event: &MsgraphEvent) -> Cow<'_, MsgraphEvent> {
    if event.is_all_day.as_option() == Some(&true) {
        return Cow::Borrowed(event);
    }

    let start = local_boundary(
        event.start.as_option(),
        event.original_start_time_zone.as_deref(),
    );
    let end = local_boundary(
        event.end.as_option(),
        event.original_end_time_zone.as_deref(),
    );

    if start.is_none() && end.is_none() {
        return Cow::Borrowed(event);
    }

    let mut event = event.clone();
    if let Some(start) = start {
        event.start = MsgraphField::Set(start);
    }
    if let Some(end) = end {
        event.end = MsgraphField::Set(end);
    }

    Cow::Owned(event)
}

/// A UTC boundary as the wall time of `original`, `None` when it is not
/// UTC or `original` is no IANA zone.
fn local_boundary(
    boundary: Option<&MsgraphDateTimeTimeZone>,
    original: Option<&str>,
) -> Option<MsgraphDateTimeTimeZone> {
    let boundary = boundary?;
    if !matches!(zone(boundary.time_zone.as_deref()), Zone::Utc) {
        return None;
    }
    let Zone::Iana(name) = zone(original) else {
        return None;
    };

    let utc = boundary
        .date_time
        .split('.')
        .next()?
        .parse::<DateTime>()
        .ok()?;
    let local = utc
        .to_zoned(TimeZone::UTC)
        .ok()?
        .with_time_zone(TimeZone::get(&name).ok()?)
        .datetime();

    Some(MsgraphDateTimeTimeZone {
        date_time: local.strftime("%Y-%m-%dT%H:%M:%S").to_string(),
        time_zone: Some(name),
    })
}

/// How Graph names a zone, resolved for iCalendar.
pub(crate) enum Zone {
    /// UTC: a `Z`-suffixed time.
    Utc,
    /// An IANA zone, given a VTIMEZONE.
    Iana(String),
    /// A name neither the Windows table nor the database knows, kept.
    Unknown(String),
}

/// Resolves a Graph zone name, Windows or IANA.
pub(crate) fn zone(name: Option<&str>) -> Zone {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Zone::Utc;
    };

    if name.eq_ignore_ascii_case("UTC") || name.eq_ignore_ascii_case("Etc/UTC") {
        return Zone::Utc;
    }

    if let Some(iana) = windows_zones::iana(name) {
        return match iana {
            "Etc/UTC" => Zone::Utc,
            iana => Zone::Iana(iana.to_owned()),
        };
    }

    if tzdb::is_known(name) {
        return Zone::Iana(name.to_owned());
    }

    Zone::Unknown(name.to_owned())
}

/// A DTSTART or DTEND from a Graph wall time in a zone.
fn boundary_prop(
    kind: IcalPropKind,
    boundary: &MsgraphDateTimeTimeZone,
    all_day: bool,
    zones: &mut BTreeSet<String>,
) -> IcalProp<'static> {
    let digits = ical_digits(&boundary.date_time);

    if all_day {
        return IcalProp {
            name: IcalPropName::Kind(kind),
            params: vec![IcalParam::Value("DATE".into())],
            value: IcalValue::Date(digits.get(..8).unwrap_or_default().to_owned().into()),
        };
    }

    match zone(boundary.time_zone.as_deref()) {
        Zone::Utc => stamp_prop(kind, format!("{digits}Z")),
        Zone::Iana(zone) | Zone::Unknown(zone) => {
            zones.insert(zone.clone());
            IcalProp {
                name: IcalPropName::Kind(kind),
                params: vec![IcalParam::TzId(zone.into())],
                value: IcalValue::DateTime(digits.into()),
            }
        }
    }
}

/// The RECURRENCE-ID of an exception: its original start, a UTC instant,
/// told in the zone of its series' start, as RFC 5545 3.8.4.4 wants it.
fn recurrence_id(
    master: &MsgraphEvent,
    original: &str,
    zones: &mut BTreeSet<String>,
) -> Option<IcalProp<'static>> {
    let instant = original.parse::<Timestamp>().ok()?;
    let all_day = master.is_all_day.as_option().copied().unwrap_or(false);
    let start_zone = master
        .start
        .as_option()
        .and_then(|start| start.time_zone.as_deref());

    if all_day {
        let date = instant.to_zoned(TimeZone::UTC).date();
        return Some(IcalProp {
            name: IcalPropName::Kind(IcalPropKind::RecurrenceId),
            params: vec![IcalParam::Value("DATE".into())],
            value: IcalValue::Date(date.strftime("%Y%m%d").to_string().into()),
        });
    }

    match zone(start_zone) {
        Zone::Iana(name) => {
            let local = instant.to_zoned(TimeZone::get(&name).ok()?).datetime();
            zones.insert(name.clone());
            Some(IcalProp {
                name: IcalPropName::Kind(IcalPropKind::RecurrenceId),
                params: vec![IcalParam::TzId(name.into())],
                value: IcalValue::DateTime(local.strftime("%Y%m%dT%H%M%S").to_string().into()),
            })
        }
        _ => Some(stamp_prop(
            IcalPropKind::RecurrenceId,
            instant.strftime("%Y%m%dT%H%M%SZ").to_string(),
        )),
    }
}

/// The RRULE of a Graph recurrence, `None` for a pattern missing what its
/// kind needs.
pub(crate) fn rrule(
    recurrence: &MsgraphPatternedRecurrence,
    start: Option<&MsgraphDateTimeTimeZone>,
    all_day: bool,
) -> Option<String> {
    let pattern = &recurrence.pattern;
    let mut parts = Vec::new();

    let days = || -> String {
        pattern
            .days_of_week
            .iter()
            .map(|day| weekday_to_ical(*day))
            .collect::<Vec<_>>()
            .join(",")
    };

    match pattern.pattern_type? {
        MsgraphRecurrencePatternType::Daily => parts.push(String::from("FREQ=DAILY")),
        MsgraphRecurrencePatternType::Weekly => {
            parts.push(String::from("FREQ=WEEKLY"));
            if !pattern.days_of_week.is_empty() {
                parts.push(format!("BYDAY={}", days()));
            }
        }
        MsgraphRecurrencePatternType::AbsoluteMonthly => {
            parts.push(String::from("FREQ=MONTHLY"));
            parts.push(format!("BYMONTHDAY={}", pattern.day_of_month?));
        }
        MsgraphRecurrencePatternType::RelativeMonthly => {
            parts.push(String::from("FREQ=MONTHLY"));
            parts.push(format!("BYDAY={}", days()));
            parts.push(format!("BYSETPOS={}", setpos(pattern.index)));
        }
        MsgraphRecurrencePatternType::AbsoluteYearly => {
            parts.push(String::from("FREQ=YEARLY"));
            parts.push(format!("BYMONTH={}", pattern.month?));
            parts.push(format!("BYMONTHDAY={}", pattern.day_of_month?));
        }
        MsgraphRecurrencePatternType::RelativeYearly => {
            parts.push(String::from("FREQ=YEARLY"));
            parts.push(format!("BYMONTH={}", pattern.month?));
            parts.push(format!("BYDAY={}", days()));
            parts.push(format!("BYSETPOS={}", setpos(pattern.index)));
        }
    }

    if let Some(interval) = pattern.interval.filter(|interval| *interval > 1) {
        parts.push(format!("INTERVAL={interval}"));
    }

    if let Some(first) = pattern.first_day_of_week
        && pattern.pattern_type == Some(MsgraphRecurrencePatternType::Weekly)
    {
        parts.push(format!("WKST={}", weekday_to_ical(first)));
    }

    let range = &recurrence.range;
    match range.range_type {
        Some(MsgraphRecurrenceRangeType::Numbered) => {
            parts.push(format!("COUNT={}", range.number_of_occurrences?));
        }
        Some(MsgraphRecurrenceRangeType::EndDate) => {
            let end = range.end_date.as_deref()?.parse::<Date>().ok()?;
            parts.push(format!("UNTIL={}", until(end, range, start, all_day)?));
        }
        Some(MsgraphRecurrenceRangeType::NoEnd) | None => {}
    }

    Some(parts.join(";"))
}

/// The UNTIL of an `endDate` range: the date itself for an all-day series,
/// else the end of that day in the recurrence zone, told in UTC as RFC 5545
/// 3.3.10 wants it beside a zoned DTSTART.
fn until(
    end: Date,
    range: &MsgraphRecurrenceRange,
    start: Option<&MsgraphDateTimeTimeZone>,
    all_day: bool,
) -> Option<String> {
    if all_day {
        return Some(end.strftime("%Y%m%d").to_string());
    }

    let name = range
        .recurrence_time_zone
        .as_deref()
        .or(start.and_then(|start| start.time_zone.as_deref()));
    let zone = match zone(name) {
        Zone::Iana(name) => TimeZone::get(&name).ok()?,
        _ => TimeZone::UTC,
    };

    let last = end.at(23, 59, 59, 0).to_zoned(zone).ok()?;
    Some(last.timestamp().strftime("%Y%m%dT%H%M%SZ").to_string())
}

/// The EXDATEs of a series' cancelled occurrences, read off their
/// occurrence ids and told at the series' start time.
fn exdates(
    master: &MsgraphEvent,
    all_day: bool,
    zones: &mut BTreeSet<String>,
) -> Vec<IcalProp<'static>> {
    let Some(start) = master.start.as_option() else {
        return Vec::new();
    };
    let time = ical_digits(&start.date_time);
    let time = time.get(8..).unwrap_or("T000000").to_owned();

    master
        .cancelled_occurrences
        .iter()
        .filter_map(|occurrence| occurrence.rsplit('.').next())
        .filter_map(|date| date.parse::<Date>().ok())
        .map(|date| {
            let day = date.strftime("%Y%m%d").to_string();
            if all_day {
                return IcalProp {
                    name: IcalPropName::Kind(IcalPropKind::ExDate),
                    params: vec![IcalParam::Value("DATE".into())],
                    value: IcalValue::Date(day.into()),
                };
            }
            match zone(start.time_zone.as_deref()) {
                Zone::Utc => stamp_prop(IcalPropKind::ExDate, format!("{day}{time}Z")),
                Zone::Iana(name) | Zone::Unknown(name) => {
                    zones.insert(name.clone());
                    IcalProp {
                        name: IcalPropName::Kind(IcalPropKind::ExDate),
                        params: vec![IcalParam::TzId(name.into())],
                        value: IcalValue::DateTime(format!("{day}{time}").into()),
                    }
                }
            }
        })
        .collect()
}

/// A parsed DTSTART or DTEND, ready for Graph.
pub(crate) struct Boundary {
    /// The wall time or the midnight of the date.
    pub(crate) local: DateTime,
    /// Whether the boundary is a date.
    pub(crate) all_day: bool,
    /// The zone, `UTC` for a `Z` time and for a date.
    pub(crate) zone: String,
}

impl Boundary {
    /// The boundary shifted by a DURATION.
    fn shifted(&self, duration: Span) -> Option<Self> {
        Some(Self {
            local: self.local.checked_add(duration).ok()?,
            all_day: self.all_day,
            zone: self.zone.clone(),
        })
    }

    /// The Graph form of the boundary.
    fn into_graph(self) -> MsgraphDateTimeTimeZone {
        MsgraphDateTimeTimeZone {
            date_time: self.local.strftime("%Y-%m-%dT%H:%M:%S").to_string(),
            time_zone: Some(self.zone),
        }
    }
}

/// Reads a DTSTART or DTEND, `None` for a floating time, which has no
/// instant Graph could store.
pub(crate) fn boundary(line: &IcalLine<'_>) -> Option<Boundary> {
    let value = line.raw_value_str();
    let value = value.trim();

    let is_date = line
        .param::<VALUE>()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("DATE"))
        || (value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()));

    if is_date {
        let date = Date::strptime("%Y%m%d", value).ok()?;
        return Some(Boundary {
            local: date.at(0, 0, 0, 0),
            all_day: true,
            zone: String::from("UTC"),
        });
    }

    let (local, zone) = match value.strip_suffix('Z') {
        Some(local) => (local, String::from("UTC")),
        None => (value, line.param::<TZID>()?.into_owned()),
    };

    Some(Boundary {
        local: DateTime::strptime("%Y%m%dT%H%M%S", local).ok()?,
        all_day: false,
        zone,
    })
}

/// The Graph recurrence of an RRULE, refused when Graph has no pattern for
/// it rather than approximated.
pub(crate) fn recurrence(
    rule: &str,
    start: &Boundary,
) -> Result<MsgraphPatternedRecurrence, MsgraphEventIcalError> {
    let unsupported = || MsgraphEventIcalError::UnsupportedRecurrence(rule.to_owned());

    let mut freq = None;
    let mut interval = None;
    let mut count = None;
    let mut until_value = None;
    let mut by_day = Vec::new();
    let mut by_month_day = None;
    let mut by_month = None;
    let mut by_setpos = None;
    let mut wkst = None;

    for part in rule.split(';').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').ok_or_else(unsupported)?;
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => freq = Some(value.to_ascii_uppercase()),
            "INTERVAL" => interval = Some(value.parse::<u32>().map_err(|_| unsupported())?),
            "COUNT" => count = Some(value.parse::<u32>().map_err(|_| unsupported())?),
            "UNTIL" => until_value = Some(value.to_owned()),
            "BYDAY" => {
                for day in value.split(',') {
                    let (ordinal, name) = day.split_at(day.len().saturating_sub(2));
                    if !ordinal.is_empty() {
                        by_setpos = Some(ordinal.parse::<i32>().map_err(|_| unsupported())?);
                    }
                    by_day.push(weekday_from_ical(name).ok_or_else(unsupported)?);
                }
            }
            "BYMONTHDAY" => by_month_day = Some(value.parse::<u32>().map_err(|_| unsupported())?),
            "BYMONTH" => by_month = Some(value.parse::<u32>().map_err(|_| unsupported())?),
            "BYSETPOS" => by_setpos = Some(value.parse::<i32>().map_err(|_| unsupported())?),
            "WKST" => wkst = Some(weekday_from_ical(value).ok_or_else(unsupported)?),
            _ => return Err(unsupported()),
        }
    }

    let index = match by_setpos {
        None => None,
        Some(1) => Some(MsgraphWeekIndex::First),
        Some(2) => Some(MsgraphWeekIndex::Second),
        Some(3) => Some(MsgraphWeekIndex::Third),
        Some(4) => Some(MsgraphWeekIndex::Fourth),
        Some(-1) => Some(MsgraphWeekIndex::Last),
        Some(_) => return Err(unsupported()),
    };

    let pattern_type = match (freq.as_deref(), by_day.is_empty(), index) {
        (Some("DAILY"), true, None) => MsgraphRecurrencePatternType::Daily,
        (Some("WEEKLY"), _, None) => MsgraphRecurrencePatternType::Weekly,
        (Some("MONTHLY"), true, None) => MsgraphRecurrencePatternType::AbsoluteMonthly,
        (Some("MONTHLY"), false, Some(_)) => MsgraphRecurrencePatternType::RelativeMonthly,
        (Some("YEARLY"), true, None) => MsgraphRecurrencePatternType::AbsoluteYearly,
        (Some("YEARLY"), false, Some(_)) => MsgraphRecurrencePatternType::RelativeYearly,
        _ => return Err(unsupported()),
    };

    let date = start.local.date();
    let mut days_of_week = by_day;
    if pattern_type == MsgraphRecurrencePatternType::Weekly && days_of_week.is_empty() {
        days_of_week.push(weekday_of(date));
    }

    let (day_of_month, month) = match pattern_type {
        MsgraphRecurrencePatternType::AbsoluteMonthly => {
            (Some(by_month_day.unwrap_or(date.day() as u32)), None)
        }
        MsgraphRecurrencePatternType::AbsoluteYearly => (
            Some(by_month_day.unwrap_or(date.day() as u32)),
            Some(by_month.unwrap_or(date.month() as u32)),
        ),
        MsgraphRecurrencePatternType::RelativeYearly => {
            (None, Some(by_month.unwrap_or(date.month() as u32)))
        }
        _ => (None, None),
    };

    let zone_name = (!start.all_day).then(|| start.zone.clone());
    let range = match (count, until_value) {
        (Some(count), _) => MsgraphRecurrenceRange {
            range_type: Some(MsgraphRecurrenceRangeType::Numbered),
            number_of_occurrences: Some(count),
            ..Default::default()
        },
        (None, Some(until)) => {
            let day = until.get(..8).ok_or_else(unsupported)?;
            let end = Date::strptime("%Y%m%d", day).map_err(|_| unsupported())?;
            MsgraphRecurrenceRange {
                range_type: Some(MsgraphRecurrenceRangeType::EndDate),
                end_date: Some(end.strftime("%Y-%m-%d").to_string()),
                ..Default::default()
            }
        }
        (None, None) => MsgraphRecurrenceRange {
            range_type: Some(MsgraphRecurrenceRangeType::NoEnd),
            ..Default::default()
        },
    };

    Ok(MsgraphPatternedRecurrence {
        pattern: MsgraphRecurrencePattern {
            pattern_type: Some(pattern_type),
            interval: Some(interval.unwrap_or(1)),
            month,
            day_of_month,
            days_of_week,
            first_day_of_week: wkst,
            index,
        },
        range: MsgraphRecurrenceRange {
            start_date: Some(date.strftime("%Y-%m-%d").to_string()),
            recurrence_time_zone: zone_name,
            ..range
        },
    })
}

/// The first VEVENT of a document that `pick` accepts: the master is the
/// one without a RECURRENCE-ID.
fn find_vevent<'a>(
    calendar: &'a IcalCst<'a>,
    pick: impl Fn(&IcalCst<'_>) -> bool,
) -> Result<&'a IcalCst<'a>, MsgraphEventIcalError> {
    let components = || {
        calendar.items.iter().filter_map(|item| match item {
            IcalItem::Component(component) => Some(component.as_ref()),
            _ => None,
        })
    };

    if let Some(vevent) = components()
        .filter(|component| component_name(component) == "VEVENT")
        .find(|vevent| pick(vevent))
    {
        return Ok(vevent);
    }

    match components()
        .map(component_name)
        .find(|name| name != "VTIMEZONE" && name != "VEVENT")
    {
        Some(name) => Err(MsgraphEventIcalError::NotAnEvent(name)),
        None => Err(MsgraphEventIcalError::NoEvent),
    }
}

/// The upper-cased name a component's BEGIN line gives it.
pub(crate) fn component_name(component: &IcalCst<'_>) -> String {
    component
        .begin
        .as_ref()
        .map(|begin| begin.raw_value_str().trim().to_uppercase())
        .unwrap_or_default()
}

/// Whether a component carries a property of that name.
fn has_prop(component: &IcalCst<'_>, name: &str) -> bool {
    component.items.iter().any(|item| match item {
        IcalItem::Prop(line) => line.name.get().eq_ignore_ascii_case(name),
        _ => false,
    })
}

/// The lead time of a DISPLAY or AUDIO alarm triggering before the start,
/// in minutes, `None` for any alarm Graph's single reminder cannot hold.
fn reminder_minutes(alarm: &IcalCst<'_>) -> Option<u32> {
    if component_name(alarm) != "VALARM" {
        return None;
    }
    let action = alarm.prop::<ACTION>()?;
    if !matches!(action.0.trim().to_uppercase().as_str(), "DISPLAY" | "AUDIO") {
        return None;
    }

    let trigger = alarm.prop::<TRIGGER>()?;
    let lead = trigger.0.trim().strip_prefix('-')?.parse::<Span>().ok()?;
    let minutes = lead.total(jiff::Unit::Minute).ok()?;

    let whole = minutes as u32;
    (minutes >= 0.0 && f64::from(whole) == minutes).then_some(whole)
}

/// An ORGANIZER or ATTENDEE value and its CN.
fn person_prop(
    kind: IcalPropKind,
    address: &MsgraphEmailAddress,
    mut params: Vec<IcalParam<'static>>,
) -> IcalProp<'static> {
    if let Some(name) = address.name.clone().filter(|name| !name.is_empty()) {
        params.insert(0, IcalParam::Cn(name.into()));
    }
    let email = address.address.clone().unwrap_or_default();

    IcalProp {
        name: IcalPropName::Kind(kind),
        params,
        value: IcalValue::CalAddress(format!("mailto:{email}").into()),
    }
}

/// An ATTENDEE from a Graph attendee, `None` without an address.
fn attendee_prop(attendee: &MsgraphAttendee) -> Option<IcalProp<'static>> {
    let address = attendee.email_address.as_ref()?;

    let mut params = Vec::new();
    match attendee.attendee_type {
        Some(MsgraphAttendeeType::Optional) => {
            params.push(IcalParam::Role("OPT-PARTICIPANT".into()))
        }
        Some(MsgraphAttendeeType::Resource) => {
            params.push(IcalParam::Role("NON-PARTICIPANT".into()));
            params.push(IcalParam::CuType("RESOURCE".into()));
        }
        _ => params.push(IcalParam::Role("REQ-PARTICIPANT".into())),
    }

    let partstat = match attendee.status.as_ref().and_then(|status| status.response) {
        Some(MsgraphResponseType::Accepted | MsgraphResponseType::Organizer) => "ACCEPTED",
        Some(MsgraphResponseType::Declined) => "DECLINED",
        Some(MsgraphResponseType::TentativelyAccepted) => "TENTATIVE",
        _ => "NEEDS-ACTION",
    };
    params.push(IcalParam::PartStat(partstat.into()));

    Some(person_prop(IcalPropKind::Attendee, address, params))
}

/// A Graph attendee from an ATTENDEE.
fn attendee(line: &IcalLine<'_>) -> MsgraphAttendee {
    let role = line.param::<ROLE>().unwrap_or_default();
    let cutype = line.param::<CUTYPE>().unwrap_or_default();

    let attendee_type =
        if cutype.eq_ignore_ascii_case("RESOURCE") || cutype.eq_ignore_ascii_case("ROOM") {
            MsgraphAttendeeType::Resource
        } else if role.eq_ignore_ascii_case("OPT-PARTICIPANT") {
            MsgraphAttendeeType::Optional
        } else {
            MsgraphAttendeeType::Required
        };

    let response =
        line.param::<PARTSTAT>()
            .and_then(|status| match status.to_ascii_uppercase().as_str() {
                "ACCEPTED" => Some(MsgraphResponseType::Accepted),
                "DECLINED" => Some(MsgraphResponseType::Declined),
                "TENTATIVE" => Some(MsgraphResponseType::TentativelyAccepted),
                "NEEDS-ACTION" => Some(MsgraphResponseType::NotResponded),
                _ => None,
            });

    let value = line.raw_value_str();
    let value = value.trim();
    let address = value
        .strip_prefix("mailto:")
        .or_else(|| value.strip_prefix("MAILTO:"))
        .unwrap_or(value)
        .to_owned();

    MsgraphAttendee {
        attendee_type: Some(attendee_type),
        status: response.map(|response| MsgraphResponseStatus {
            response: Some(response),
            time: None,
        }),
        email_address: Some(MsgraphEmailAddress {
            name: line.param::<CN>().map(Cow::into_owned),
            address: Some(address),
        }),
    }
}

/// The time-zone anchor of a series, its start date in Unix seconds.
fn anchor(event: &MsgraphEvent) -> i64 {
    event
        .start
        .as_option()
        .and_then(|start| start.date_time.get(..10))
        .and_then(|date| date.parse::<Date>().ok())
        .and_then(|date| date.to_zoned(TimeZone::UTC).ok())
        .map_or(0, |zoned| zoned.timestamp().as_second())
}

/// A Graph date-time, `YYYY-MM-DDTHH:MM:SS[.fffffff]`, as iCalendar digits
/// `YYYYMMDDTHHMMSS`.
fn ical_digits(date_time: &str) -> String {
    let base = date_time.split('.').next().unwrap_or(date_time);
    base.chars()
        .filter(|character| *character != '-' && *character != ':')
        .take(15)
        .collect()
}

/// An RFC 3339 instant as an iCalendar UTC stamp.
pub(crate) fn utc_stamp(instant: &str) -> Option<String> {
    let instant = instant.parse::<Timestamp>().ok()?;
    Some(instant.strftime("%Y%m%dT%H%M%SZ").to_string())
}

/// A UTC date-time property.
pub(crate) fn stamp_prop(kind: IcalPropKind, stamp: String) -> IcalProp<'static> {
    IcalProp {
        name: IcalPropName::Kind(kind),
        params: Vec::new(),
        value: IcalValue::DateTime(IcalDateTime(stamp.into())),
    }
}

/// The text a body says, whatever its content type: Exchange turns every
/// body into HTML, so the type alone never tells an edit.
pub(crate) fn body_text(body: &MsgraphField<MsgraphItemBody>) -> String {
    let Some(body) = body.as_option() else {
        return String::new();
    };
    let content = body.content.as_deref().unwrap_or_default();
    match body.content_type {
        Some(MsgraphBodyType::Html) => strip_html(content),
        _ => content.to_owned(),
    }
}

/// The text of an HTML body without its markup, for DESCRIPTION.
pub(crate) fn strip_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_owned()
}

/// Whether two texts say the same once their whitespace is collapsed, so
/// line endings and indentation the HTML source carried do not count.
pub(crate) fn same_text(left: &str, right: &str) -> bool {
    left.split_whitespace().eq(right.split_whitespace())
}

/// The stashed lines behind an item's extended properties, matched by name.
pub(crate) fn stash_lines(
    props: &MsgraphField<Vec<MsgraphSingleValueExtendedProperty>>,
) -> Vec<String> {
    props
        .as_option()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter(|prop| prop.id.to_ascii_lowercase().contains("pimalaya-ical"))
        .flat_map(|prop| prop.value.split('\n'))
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Restores stashed lines into `component`: each property alone, so one
/// that no longer tokenises restores nothing rather than corrupting the
/// item, and each stashed subcomponent in one go, so its lines stay
/// together.
pub(crate) fn push_stash(component: &mut IcalCst<'_>, lines: &[String]) {
    let mut block: Vec<&str> = Vec::new();
    let mut depth = 0_usize;

    for line in lines {
        let name = line.split([';', ':']).next().unwrap_or_default();
        if name.eq_ignore_ascii_case("BEGIN") {
            depth += 1;
        }

        if depth == 0 {
            let _ = component.push_raw(line);
            continue;
        }

        block.push(line);
        if name.eq_ignore_ascii_case("END") {
            depth -= 1;
            if depth == 0 {
                let _ = component.push_raw(&block.join("\r\n"));
                block.clear();
            }
        }
    }
}

/// Whether a stashed line is the UID, written by [`vevent`] itself.
fn is_uid_line(line: &str) -> bool {
    let name = line.split([';', ':']).next().unwrap_or_default();
    name.eq_ignore_ascii_case("UID")
}

/// Whether a stashed line is a property minted from Graph-scoped fields.
fn is_minted_line(line: &str) -> bool {
    let name = line.split([';', ':']).next().unwrap_or_default();
    MINTED_PROPS
        .iter()
        .any(|minted| name.eq_ignore_ascii_case(minted))
}

/// The decoded text of a property line.
pub(crate) fn text(line: &IcalLine<'_>) -> String {
    IcalText::decode(&line.value).0.into_owned()
}

/// One logical line, its ending stripped, for the stash.
pub(crate) fn raw_line(line: &IcalLine<'_>) -> String {
    line.to_string().trim_end_matches(['\r', '\n']).to_owned()
}

/// A whole component as its raw lines, endings stripped.
pub(crate) fn raw_component(component: &IcalCst<'_>) -> Vec<String> {
    component
        .to_string()
        .lines()
        .map(|line| line.trim_end_matches('\r').to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

/// The BYSETPOS of a relative pattern's week index.
fn setpos(index: Option<MsgraphWeekIndex>) -> i32 {
    match index {
        Some(MsgraphWeekIndex::Second) => 2,
        Some(MsgraphWeekIndex::Third) => 3,
        Some(MsgraphWeekIndex::Fourth) => 4,
        Some(MsgraphWeekIndex::Last) => -1,
        _ => 1,
    }
}

/// The iCalendar weekday of a Graph day.
fn weekday_to_ical(day: MsgraphDayOfWeek) -> &'static str {
    match day {
        MsgraphDayOfWeek::Sunday => "SU",
        MsgraphDayOfWeek::Monday => "MO",
        MsgraphDayOfWeek::Tuesday => "TU",
        MsgraphDayOfWeek::Wednesday => "WE",
        MsgraphDayOfWeek::Thursday => "TH",
        MsgraphDayOfWeek::Friday => "FR",
        MsgraphDayOfWeek::Saturday => "SA",
    }
}

/// The Graph day of an iCalendar weekday.
fn weekday_from_ical(day: &str) -> Option<MsgraphDayOfWeek> {
    Some(match day.to_ascii_uppercase().as_str() {
        "SU" => MsgraphDayOfWeek::Sunday,
        "MO" => MsgraphDayOfWeek::Monday,
        "TU" => MsgraphDayOfWeek::Tuesday,
        "WE" => MsgraphDayOfWeek::Wednesday,
        "TH" => MsgraphDayOfWeek::Thursday,
        "FR" => MsgraphDayOfWeek::Friday,
        "SA" => MsgraphDayOfWeek::Saturday,
        _ => return None,
    })
}

/// The Graph day a date falls on.
fn weekday_of(date: Date) -> MsgraphDayOfWeek {
    use jiff::civil::Weekday;
    match date.weekday() {
        Weekday::Sunday => MsgraphDayOfWeek::Sunday,
        Weekday::Monday => MsgraphDayOfWeek::Monday,
        Weekday::Tuesday => MsgraphDayOfWeek::Tuesday,
        Weekday::Wednesday => MsgraphDayOfWeek::Wednesday,
        Weekday::Thursday => MsgraphDayOfWeek::Thursday,
        Weekday::Friday => MsgraphDayOfWeek::Friday,
        Weekday::Saturday => MsgraphDayOfWeek::Saturday,
    }
}

/// The `X-MICROSOFT-CDO-BUSYSTATUS` value of a `showAs`.
fn busy_status_to_ical(show_as: MsgraphFreeBusyStatus) -> &'static str {
    match show_as {
        MsgraphFreeBusyStatus::Free => "FREE",
        MsgraphFreeBusyStatus::Tentative => "TENTATIVE",
        MsgraphFreeBusyStatus::Busy => "BUSY",
        MsgraphFreeBusyStatus::Oof => "OOF",
        MsgraphFreeBusyStatus::WorkingElsewhere => "WORKINGELSEWHERE",
        MsgraphFreeBusyStatus::Unknown => "UNKNOWN",
    }
}

/// The `showAs` of an `X-MICROSOFT-CDO-BUSYSTATUS` value.
fn busy_status_from_ical(value: &str) -> Option<MsgraphFreeBusyStatus> {
    Some(match value.trim().to_ascii_uppercase().as_str() {
        "FREE" => MsgraphFreeBusyStatus::Free,
        "TENTATIVE" => MsgraphFreeBusyStatus::Tentative,
        "BUSY" => MsgraphFreeBusyStatus::Busy,
        "OOF" => MsgraphFreeBusyStatus::Oof,
        "WORKINGELSEWHERE" => MsgraphFreeBusyStatus::WorkingElsewhere,
        _ => return None,
    })
}

/// The CLASS of a sensitivity; `personal` has none and reads as private.
fn sensitivity_to_ical(sensitivity: MsgraphSensitivity) -> &'static str {
    match sensitivity {
        MsgraphSensitivity::Normal => "PUBLIC",
        MsgraphSensitivity::Personal | MsgraphSensitivity::Private => "PRIVATE",
        MsgraphSensitivity::Confidential => "CONFIDENTIAL",
    }
}

/// The sensitivity of a CLASS.
fn sensitivity_from_ical(value: &str) -> Option<MsgraphSensitivity> {
    Some(match value.trim().to_ascii_uppercase().as_str() {
        "PUBLIC" => MsgraphSensitivity::Normal,
        "PRIVATE" => MsgraphSensitivity::Private,
        "CONFIDENTIAL" => MsgraphSensitivity::Confidential,
        _ => return None,
    })
}

/// The importance of a PRIORITY: 1 to 4 high, 5 normal, 6 to 9 low, 0 none.
pub(crate) fn importance_from_ical(value: &str) -> Option<MsgraphImportance> {
    match value.trim().parse::<u8>().ok()? {
        1..=4 => Some(MsgraphImportance::High),
        5 => Some(MsgraphImportance::Normal),
        6..=9 => Some(MsgraphImportance::Low),
        _ => None,
    }
}

/// The class id that opens every Exchange global object id
/// ([MS-OXOCAL] §2.2.1.27, `PidLidGlobalObjectId`), hex-encoded.
const GLOBAL_OBJECT_ID_CLASS: &str = "040000008200E00074C5B7101A82E008";

/// The marker of a global object id that wraps an iCalendar UID Exchange
/// did not mint ([MS-OXCICAL] §2.1.3.1.1.20.26).
const VCAL_UID_MARKER: &[u8] = b"vCal-Uid\x01\x00\x00\x00";

/// The iCalendar UID of a Graph `iCalUId`.
///
/// An invitation Exchange took in by mail is filed under its global object
/// id, which wraps the organizer's UID (the `vCal-Uid` form) rather than
/// keeping it: the wrapped UID is returned, so the event keeps the UID the
/// organizer and every other attendee know it by. A global object id
/// Exchange minted itself, with no UID inside, and any other value come
/// back as they are.
pub fn original_uid(ical_uid: &str) -> String {
    unwrap_global_object_id(ical_uid).unwrap_or_else(|| ical_uid.to_owned())
}

fn unwrap_global_object_id(ical_uid: &str) -> Option<String> {
    let class = ical_uid.get(..GLOBAL_OBJECT_ID_CLASS.len())?;
    if !class.eq_ignore_ascii_case(GLOBAL_OBJECT_ID_CLASS) {
        return None;
    }
    let rest = &ical_uid.as_bytes()[GLOBAL_OBJECT_ID_CLASS.len()..];
    if !rest.len().is_multiple_of(2) {
        return None;
    }
    let bytes = rest
        .chunks(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).ok()?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    // After the class id: the instance date (4 bytes), the creation time
    // (8), reserved (8), the size of the data (4), then the data.
    let size = u32::from_le_bytes(bytes.get(20..24)?.try_into().ok()?) as usize;
    let data = bytes.get(24..24usize.checked_add(size)?)?;
    let uid = data.strip_prefix(VCAL_UID_MARKER)?;
    let end = uid.iter().position(|byte| *byte == 0).unwrap_or(uid.len());
    String::from_utf8(uid[..end].to_vec())
        .ok()
        .filter(|uid| !uid.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1::rest::users::messages::MsgraphRecipient;

    /// A weekly series in a Windows-named zone, as Outlook creates one.
    fn master() -> MsgraphEvent {
        MsgraphEvent {
            id: "SM1".into(),
            ical_uid: Some("040000008200E0".into()),
            last_modified_date_time: Some("2026-08-01T10:00:00Z".into()),
            subject: MsgraphField::Set("Stand-up".into()),
            start: MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: "2026-08-14T09:00:00.0000000".into(),
                time_zone: Some("Romance Standard Time".into()),
            }),
            end: MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: "2026-08-14T09:15:00.0000000".into(),
                time_zone: Some("Romance Standard Time".into()),
            }),
            is_all_day: MsgraphField::Set(false),
            recurrence: MsgraphField::Set(MsgraphPatternedRecurrence {
                pattern: MsgraphRecurrencePattern {
                    pattern_type: Some(MsgraphRecurrencePatternType::Weekly),
                    interval: Some(1),
                    days_of_week: vec![MsgraphDayOfWeek::Monday, MsgraphDayOfWeek::Friday],
                    first_day_of_week: Some(MsgraphDayOfWeek::Monday),
                    ..Default::default()
                },
                range: MsgraphRecurrenceRange {
                    range_type: Some(MsgraphRecurrenceRangeType::Numbered),
                    start_date: Some("2026-08-14".into()),
                    number_of_occurrences: Some(10),
                    ..Default::default()
                },
            }),
            organizer: MsgraphField::Set(MsgraphRecipient {
                email_address: MsgraphEmailAddress {
                    name: Some("Alice".into()),
                    address: Some("alice@x.org".into()),
                },
            }),
            attendees: MsgraphField::Set(vec![MsgraphAttendee {
                attendee_type: Some(MsgraphAttendeeType::Optional),
                status: Some(MsgraphResponseStatus {
                    response: Some(MsgraphResponseType::Accepted),
                    time: None,
                }),
                email_address: Some(MsgraphEmailAddress {
                    name: Some("Bob".into()),
                    address: Some("bob@x.org".into()),
                }),
            }]),
            show_as: MsgraphField::Set(MsgraphFreeBusyStatus::Tentative),
            is_reminder_on: MsgraphField::Set(true),
            reminder_minutes_before_start: MsgraphField::Set(15),
            cancelled_occurrences: vec!["OID.SM1.2026-08-21".into()],
            ..Default::default()
        }
    }

    #[test]
    fn a_stashed_alarm_reads_back_whole() {
        let doc = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\n\
            DTSTART:20261120T090000Z\r\nDTEND:20261120T100000Z\r\nSUMMARY:Review\r\n\
            BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n\
            BEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT1H\r\nSUMMARY:Soon\r\nEND:VALARM\r\n\
            END:VEVENT\r\nEND:VCALENDAR\r\n";

        let ical = MsgraphEvent::from_ical(doc.as_bytes()).unwrap().to_ical();

        assert!(ical.contains(
            "SUMMARY:Review\r\nBEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT1H\r\nSUMMARY:Soon\r\n\
             END:VALARM\r\n"
        ));
    }

    #[test]
    fn a_windows_zoned_series_projects_with_its_rule_and_zone() {
        let ical = master().to_ical();

        assert!(ical.contains("UID:040000008200E0\r\n"), "{ical}");
        assert!(
            ical.contains("BEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains("DTSTART;TZID=Europe/Paris:20260814T090000\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains("DTEND;TZID=Europe/Paris:20260814T091500\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains("RRULE:FREQ=WEEKLY;BYDAY=MO,FR;WKST=MO;COUNT=10\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains("EXDATE;TZID=Europe/Paris:20260821T090000\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains("ORGANIZER;CN=Alice:mailto:alice@x.org\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains(
                "ATTENDEE;CN=Bob;ROLE=OPT-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:bob@x.org\r\n"
            ),
            "{ical}"
        );
        assert!(
            ical.contains("X-MICROSOFT-CDO-BUSYSTATUS:TENTATIVE\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains("BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\n"),
            "{ical}"
        );
    }

    #[test]
    fn an_exception_carries_the_series_uid_and_its_original_start() {
        let exception = MsgraphEvent {
            id: "EX1".into(),
            ical_uid: Some("other".into()),
            event_type: Some(crate::v1::rest::users::events::MsgraphEventType::Exception),
            series_master_id: Some("SM1".into()),
            // NOTE: 07:00 UTC is 09:00 in Paris in summer.
            original_start: Some("2026-08-17T07:00:00Z".into()),
            subject: MsgraphField::Set("Stand-up, moved".into()),
            start: MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: "2026-08-17T10:00:00.0000000".into(),
                time_zone: Some("Romance Standard Time".into()),
            }),
            ..Default::default()
        };

        let ical = master().to_ical_series(&[&exception]);

        assert_eq!(ical.matches("BEGIN:VEVENT").count(), 2, "{ical}");
        assert_eq!(ical.matches("UID:040000008200E0\r\n").count(), 2, "{ical}");
        assert!(
            ical.contains("RECURRENCE-ID;TZID=Europe/Paris:20260817T090000\r\n"),
            "{ical}"
        );
        assert_eq!(ical.matches("RRULE:FREQ=WEEKLY").count(), 1, "{ical}");
    }

    /// A Google invitation as Microsoft 365 filed it, seen live: its global
    /// object id wraps the organizer's UID.
    const WRAPPED: &str = "040000008200E00074C5B7101A82E00800000000000000000000000000000000000000002D0000007643616C2D556964010000006D6F612D393839663030616435646236393839663030616435646236406D6F6100";

    #[test]
    fn a_global_object_id_gives_back_the_uid_it_wraps() {
        let uid = "moa-989f00ad5db6989f00ad5db6@moa";
        assert_eq!(original_uid(WRAPPED), uid);
        assert_eq!(original_uid(&WRAPPED.to_lowercase()), uid);

        // NOTE: Exchange's own id, nothing wrapped inside.
        let minted = "040000008200E00074C5B7101A82E0080000000088DCD2137A53DD01000000000000000010000000F70EDDABBC4DD5439803DF5489114F0B";
        assert_eq!(original_uid(minted), minted);
        assert_eq!(original_uid("040000008200E0"), "040000008200E0");
        assert_eq!(original_uid("urn:uuid:4fbe8971"), "urn:uuid:4fbe8971");
        // NOTE: a size running past the end is not trusted.
        assert_eq!(
            original_uid(&WRAPPED[..WRAPPED.len() - 4]),
            &WRAPPED[..WRAPPED.len() - 4]
        );
    }

    #[test]
    fn an_invitation_taken_in_by_mail_keeps_the_organizer_uid() {
        let mut event = master();
        event.ical_uid = Some(WRAPPED.into());
        let exception = MsgraphEvent {
            id: "EX1".into(),
            ical_uid: Some(WRAPPED.into()),
            event_type: Some(crate::v1::rest::users::events::MsgraphEventType::Exception),
            series_master_id: Some("SM1".into()),
            original_start: Some("2026-08-17T07:00:00Z".into()),
            ..Default::default()
        };

        let ical = event.to_ical_series(&[&exception]);

        assert_eq!(
            ical.matches("UID:moa-989f00ad5db6989f00ad5db6@moa\r\n")
                .count(),
            2,
            "{ical}"
        );
    }

    #[test]
    fn an_occurrence_is_told_by_the_recurrence_id_the_projection_writes() {
        // NOTE: 07:00 UTC is 09:00 in Paris in summer.
        assert_eq!(
            master().recurrence_id_of("2026-08-17T07:00:00Z").as_deref(),
            Some("20260817T090000")
        );
        assert_eq!(
            master()
                .recurrence_id_of("2026-08-17T07:00:00.0000000Z")
                .as_deref(),
            Some("20260817T090000")
        );

        let mut all_day = master();
        all_day.is_all_day = MsgraphField::Set(true);
        assert_eq!(
            all_day.recurrence_id_of("2026-08-17T00:00:00Z").as_deref(),
            Some("20260817")
        );

        assert_eq!(master().recurrence_id_of("not a time"), None);
    }

    #[test]
    fn an_all_day_event_projects_as_dates_and_back() {
        let event = MsgraphEvent {
            id: "AD1".into(),
            subject: MsgraphField::Set("Holiday".into()),
            is_all_day: MsgraphField::Set(true),
            start: MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: "2026-08-14T00:00:00.0000000".into(),
                time_zone: Some("UTC".into()),
            }),
            end: MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: "2026-08-15T00:00:00.0000000".into(),
                time_zone: Some("UTC".into()),
            }),
            ..Default::default()
        };

        let ical = event.to_ical();
        assert!(ical.contains("DTSTART;VALUE=DATE:20260814\r\n"), "{ical}");
        assert!(ical.contains("DTEND;VALUE=DATE:20260815\r\n"), "{ical}");

        let back = MsgraphEvent::from_ical(ical.as_bytes()).unwrap();
        assert_eq!(back.is_all_day.as_option(), Some(&true));
        assert_eq!(
            back.start.as_option().map(|start| start.date_time.as_str()),
            Some("2026-08-14T00:00:00")
        );
    }

    #[test]
    fn a_series_reads_back_with_its_recurrence() {
        let event = MsgraphEvent::from_ical(master().to_ical().as_bytes()).unwrap();

        assert_eq!(
            event.subject.as_option().map(String::as_str),
            Some("Stand-up")
        );
        let start = event.start.as_option().unwrap();
        assert_eq!(start.date_time, "2026-08-14T09:00:00");
        assert_eq!(start.time_zone.as_deref(), Some("Europe/Paris"));

        let recurrence = event.recurrence.as_option().unwrap();
        assert_eq!(
            recurrence.pattern.pattern_type,
            Some(MsgraphRecurrencePatternType::Weekly)
        );
        assert_eq!(
            recurrence.pattern.days_of_week,
            [MsgraphDayOfWeek::Monday, MsgraphDayOfWeek::Friday]
        );
        assert_eq!(recurrence.range.number_of_occurrences, Some(10));
        assert_eq!(recurrence.range.start_date.as_deref(), Some("2026-08-14"));
        assert_eq!(
            event.show_as.as_option(),
            Some(&MsgraphFreeBusyStatus::Tentative)
        );
        assert_eq!(event.reminder_minutes_before_start.as_option(), Some(&15));
        assert_eq!(event.attendees.as_option().map(Vec::len), Some(1));
        // NOTE: minted from the cancelled occurrence, not stashed.
        assert!(
            !stash_lines(&event.single_value_extended_properties)
                .iter()
                .any(|line| line.starts_with("EXDATE")),
            "{:?}",
            stash_lines(&event.single_value_extended_properties)
        );
    }

    #[test]
    fn a_relative_monthly_rule_round_trips_through_its_index() {
        let ical = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:u1\r\n",
            "DTSTART:20260827T120000Z\r\nDTEND:20260827T130000Z\r\n",
            "RRULE:FREQ=MONTHLY;BYDAY=TH;BYSETPOS=-1\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );

        let event = MsgraphEvent::from_ical(ical.as_bytes()).unwrap();
        let recurrence = event.recurrence.as_option().unwrap();
        assert_eq!(
            recurrence.pattern.pattern_type,
            Some(MsgraphRecurrencePatternType::RelativeMonthly)
        );
        assert_eq!(recurrence.pattern.index, Some(MsgraphWeekIndex::Last));

        let back = event.to_ical();
        assert!(
            back.contains("RRULE:FREQ=MONTHLY;BYDAY=TH;BYSETPOS=-1\r\n"),
            "{back}"
        );
    }

    #[test]
    fn an_end_date_ends_at_midnight_in_the_series_zone() {
        let mut event = master();
        if let MsgraphField::Set(recurrence) = &mut event.recurrence {
            recurrence.range = MsgraphRecurrenceRange {
                range_type: Some(MsgraphRecurrenceRangeType::EndDate),
                start_date: Some("2026-08-14".into()),
                end_date: Some("2026-09-30".into()),
                recurrence_time_zone: Some("Romance Standard Time".into()),
                ..Default::default()
            };
        }

        // NOTE: the end of 30 September in Paris, UTC+2 in summer.
        assert!(event.to_ical().contains(";UNTIL=20260930T215959Z\r\n"));
    }

    #[test]
    fn a_series_is_bounded_by_its_range_type_not_the_sentinel_end() {
        let bounds = |range_type, interval| {
            let recurrence = MsgraphPatternedRecurrence {
                pattern: MsgraphRecurrencePattern {
                    pattern_type: Some(MsgraphRecurrencePatternType::Weekly),
                    interval,
                    ..Default::default()
                },
                range: MsgraphRecurrenceRange {
                    range_type: Some(range_type),
                    start_date: Some("2026-09-28".into()),
                    end_date: Some("0001-01-01".into()),
                    number_of_occurrences: Some(11),
                    ..Default::default()
                },
            };
            recurrence
                .bounds()
                .map(|(start, end)| (start.to_string(), end.map(|end| end.to_string())))
        };

        let start = String::from("2026-09-28");
        assert_eq!(
            bounds(MsgraphRecurrenceRangeType::Numbered, None),
            Some((start.clone(), Some("2026-12-14".into()))),
        );
        assert_eq!(
            bounds(MsgraphRecurrenceRangeType::Numbered, Some(2)),
            Some((start.clone(), Some("2027-03-01".into()))),
        );
        assert_eq!(
            bounds(MsgraphRecurrenceRangeType::NoEnd, None),
            Some((start.clone(), None)),
        );
        assert_eq!(
            bounds(MsgraphRecurrenceRangeType::EndDate, None),
            Some((start, Some("0001-01-01".into()))),
        );
    }

    #[test]
    fn what_graph_cannot_hold_is_refused_by_name() {
        let with = |props: &str| {
            format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:u1\r\n{props}END:VEVENT\r\nEND:VCALENDAR\r\n"
            )
        };

        let hourly =
            with("DTSTART:20260814T090000Z\r\nDTEND:20260814T100000Z\r\nRRULE:FREQ=HOURLY\r\n");
        let floating = with("DTSTART:20260814T090000\r\nDTEND:20260814T100000\r\n");
        let todo = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:t\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        assert!(matches!(
            MsgraphEvent::from_ical(hourly.as_bytes()),
            Err(MsgraphEventIcalError::UnsupportedRecurrence(_))
        ));
        assert!(matches!(
            MsgraphEvent::from_ical(floating.as_bytes()),
            Err(MsgraphEventIcalError::NoZonedStart)
        ));
        assert!(matches!(
            MsgraphEvent::from_ical(todo.as_bytes()),
            Err(MsgraphEventIcalError::NotAnEvent(name)) if name == "VTODO"
        ));
    }

    #[test]
    fn a_duration_gives_the_end() {
        let ical = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:u1\r\n",
            "DTSTART;TZID=Europe/Paris:20260814T090000\r\nDURATION:PT1H30M\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );

        let event = MsgraphEvent::from_ical(ical.as_bytes()).unwrap();
        let end = event.end.as_option().unwrap();
        assert_eq!(end.date_time, "2026-08-14T10:30:00");
        assert_eq!(end.time_zone.as_deref(), Some("Europe/Paris"));
    }

    #[test]
    fn the_uid_and_the_remainder_ride_the_stash() {
        let ical = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:urn:uuid:4fbe8971\r\n",
            "DTSTART:20260814T090000Z\r\nDTEND:20260814T100000Z\r\nSUMMARY:Lunch\r\n",
            "X-CUSTOM;X-P=1:kept\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        );

        let mut event = MsgraphEvent::create_from_ical(ical.as_bytes()).unwrap();
        assert_eq!(event.stashed_uid().as_deref(), Some("urn:uuid:4fbe8971"));

        // NOTE: what Graph hands back after the create: its own ids, and
        // the stash it was given.
        event.id = "EV9".into();
        event.ical_uid = Some("040000008200E0".into());
        let back = event.to_ical();

        assert!(back.contains("UID:urn:uuid:4fbe8971\r\n"), "{back}");
        assert!(!back.contains("UID:040000008200E0"), "{back}");
        assert!(back.contains("X-CUSTOM;X-P=1:kept\r\n"), "{back}");
    }

    #[test]
    fn the_join_link_of_an_online_meeting_is_minted_read_only() {
        let mut event = master();
        event.online_meeting = Some(crate::v1::rest::users::events::MsgraphOnlineMeetingInfo {
            join_url: Some("https://teams.x.org/l/1".into()),
        });
        // NOTE: a stash written before the property was minted.
        event.single_value_extended_properties =
            MsgraphField::Set(vec![MsgraphSingleValueExtendedProperty {
                id: MSGRAPH_EVENT_STASH_ID.into(),
                value: "X-MICROSOFT-SKYPETEAMSMEETINGURL:https://teams.x.org/l/0".into(),
            }]);
        let ical = event.to_ical();

        assert!(
            ical.contains("CONFERENCE;VALUE=URI;FEATURE=AUDIO,VIDEO:https://teams.x.org/l/1\r\n"),
            "{ical}"
        );
        assert!(!ical.contains("SKYPETEAMS"), "{ical}");
        assert!(!ical.contains("teams.x.org/l/0"), "{ical}");

        let back = MsgraphEvent::from_ical(ical.as_bytes()).unwrap();
        assert!(back.online_meeting.is_none());
        assert!(back.is_online_meeting.is_unset());
        assert!(
            !stash_lines(&back.single_value_extended_properties)
                .iter()
                .any(|line| line.contains("CONFERENCE") || line.contains("SKYPETEAMS")),
            "{back:?}"
        );
    }

    #[test]
    fn an_online_meeting_asked_for_sets_is_online_meeting_once() {
        let base = master().to_ical();
        let asked = base.replace(
            "SUMMARY:Stand-up\r\n",
            "SUMMARY:Stand-up\r\nX-PIMDIR-ONLINE-MEETING:TRUE\r\n",
        );
        assert_ne!(asked, base);

        let created = MsgraphEvent::from_ical(asked.as_bytes()).unwrap();
        assert_eq!(created.is_online_meeting, MsgraphField::Set(true));
        assert!(
            !stash_lines(&created.single_value_extended_properties)
                .iter()
                .any(|line| line.contains("ONLINE-MEETING")),
            "{created:?}"
        );
        let body = serde_json::to_value(&created).unwrap();
        assert_eq!(body["isOnlineMeeting"], true);

        let patch = MsgraphEvent::update_from_ical(asked.as_bytes(), base.as_bytes()).unwrap();
        assert_eq!(patch.is_online_meeting, MsgraphField::Set(true));

        // NOTE: a later edit of a document still asking leaves it alone.
        let edited = asked.replace("SUMMARY:Stand-up", "SUMMARY:Daily");
        let patch = MsgraphEvent::update_from_ical(edited.as_bytes(), asked.as_bytes()).unwrap();
        assert!(patch.is_online_meeting.is_unset());

        let declined = base.replace(
            "SUMMARY:Stand-up\r\n",
            "SUMMARY:Stand-up\r\nX-PIMDIR-ONLINE-MEETING:FALSE\r\n",
        );
        let event = MsgraphEvent::from_ical(declined.as_bytes()).unwrap();
        assert!(event.is_online_meeting.is_unset());
        let body = serde_json::to_value(&event).unwrap();
        assert!(body.get("isOnlineMeeting").is_none());
    }

    #[test]
    fn an_update_sends_only_what_changed() {
        let base = master().to_ical();
        let edited = base.replace("SUMMARY:Stand-up", "SUMMARY:Daily");

        let patch = MsgraphEvent::update_from_ical(edited.as_bytes(), base.as_bytes()).unwrap();

        assert_eq!(patch.subject.as_option().map(String::as_str), Some("Daily"));
        assert!(patch.start.is_unset());
        assert!(patch.recurrence.is_unset());
        assert!(patch.attendees.is_unset());
        assert!(patch.single_value_extended_properties.is_unset());
    }

    /// One occurrence of the Monday stand-up, on its own: `hour` is when
    /// it starts, and `extra` what else its VEVENT carries.
    fn occurrence(hour: &str, summary: &str, extra: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\n\
             UID:040000008200E0\r\nRECURRENCE-ID;TZID=Europe/Paris:20260817T090000\r\n\
             DTSTART;TZID=Europe/Paris:20260817T{hour}0000\r\n\
             DTEND;TZID=Europe/Paris:20260817T{hour}1500\r\n\
             SUMMARY:{summary}\r\n{extra}END:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    }

    #[test]
    fn an_occurrence_moved_alone_sends_its_new_times() {
        let base = occurrence("09", "Stand-up", "");
        let moved = occurrence("10", "Stand-up", "");

        let patch = MsgraphEvent::instance_update_from_ical(moved.as_bytes(), base.as_bytes())
            .unwrap()
            .unwrap();

        let start = patch.start.as_option().unwrap();
        assert_eq!(start.date_time, "2026-08-17T10:00:00");
        assert_eq!(start.time_zone.as_deref(), Some("Europe/Paris"));
        assert_eq!(
            patch.end.as_option().unwrap().date_time,
            "2026-08-17T10:15:00"
        );
        assert!(patch.subject.is_unset());
        assert!(patch.body.is_unset());
        assert!(patch.attendees.is_unset());
        assert!(patch.recurrence.is_unset());
        assert!(patch.single_value_extended_properties.is_unset());
    }

    #[test]
    fn a_reverted_occurrence_takes_the_series_fields_back() {
        let exception = occurrence("10", "Stand-up, moved", "");
        let plain = occurrence("09", "Stand-up", "");

        let patch = MsgraphEvent::instance_update_from_ical(plain.as_bytes(), exception.as_bytes())
            .unwrap()
            .unwrap();

        assert_eq!(
            patch.subject.as_option().map(String::as_str),
            Some("Stand-up")
        );
        assert_eq!(
            patch.start.as_option().unwrap().date_time,
            "2026-08-17T09:00:00"
        );
    }

    /// The series' own component carried to an occurrence keeps its rule
    /// and its remainder, which belong to the master.
    #[test]
    fn an_occurrence_as_graph_holds_it_sends_nothing() {
        let held = occurrence("09", "Stand-up", "");
        let carried = occurrence("09", "Stand-up", "RRULE:FREQ=WEEKLY\r\nX-OWN:kept\r\n");

        assert_eq!(
            MsgraphEvent::instance_update_from_ical(held.as_bytes(), held.as_bytes()).unwrap(),
            None
        );
        assert_eq!(
            MsgraphEvent::instance_update_from_ical(carried.as_bytes(), held.as_bytes()).unwrap(),
            None
        );
    }

    /// A read Graph answered in UTC, as it does unless asked for a zone.
    fn utc_read() -> MsgraphEvent {
        let utc = |date_time: &str| {
            MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: date_time.into(),
                time_zone: Some("UTC".into()),
            })
        };

        MsgraphEvent {
            id: "E1".into(),
            start: utc("2030-01-10T09:00:00.0000000"),
            end: utc("2030-01-10T10:00:00.0000000"),
            is_all_day: MsgraphField::Set(false),
            original_start_time_zone: Some("Romance Standard Time".into()),
            original_end_time_zone: Some("Romance Standard Time".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_utc_read_is_told_in_its_original_zone() {
        let ical = utc_read().to_ical();

        assert!(
            ical.contains("DTSTART;TZID=Europe/Paris:20300110T100000\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains("DTEND;TZID=Europe/Paris:20300110T110000\r\n"),
            "{ical}"
        );
        assert!(
            ical.contains("BEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\n"),
            "{ical}"
        );
    }

    #[test]
    fn an_unknown_original_zone_stays_utc() {
        let mut event = utc_read();
        event.original_start_time_zone = Some("tzone://Microsoft/Custom".into());
        event.original_end_time_zone = Some("tzone://Microsoft/Custom".into());

        let ical = event.to_ical();

        assert!(ical.contains("DTSTART:20300110T090000Z\r\n"), "{ical}");
    }

    #[test]
    fn an_all_day_read_keeps_its_dates() {
        let mut event = utc_read();
        event.is_all_day = MsgraphField::Set(true);
        event.start = MsgraphField::Set(MsgraphDateTimeTimeZone {
            date_time: "2030-01-10T00:00:00.0000000".into(),
            time_zone: Some("UTC".into()),
        });
        event.end = MsgraphField::Set(MsgraphDateTimeTimeZone {
            date_time: "2030-01-11T00:00:00.0000000".into(),
            time_zone: Some("UTC".into()),
        });

        let ical = event.to_ical();

        assert!(ical.contains("DTSTART;VALUE=DATE:20300110\r\n"), "{ical}");
    }

    /// A read whose text body Exchange turned into HTML, as it always does.
    fn html_read() -> String {
        let mut event = utc_read();
        event.body = MsgraphField::Set(MsgraphItemBody {
            content_type: Some(MsgraphBodyType::Html),
            content: Some(
                "<html><body>\r\n<div class=\"PlainText\">old notes</div>\r\n</body></html>".into(),
            ),
        });
        event.to_ical()
    }

    #[test]
    fn an_untouched_description_keeps_the_html() {
        let read = html_read();

        let event = MsgraphEvent::from_ical(read.as_bytes()).unwrap();

        let body = event.body.as_option().unwrap();
        assert_eq!(body.content_type, Some(MsgraphBodyType::Html));
    }

    #[test]
    fn an_edited_description_beats_its_stale_html() {
        let read = html_read();
        let edited = read.replace("DESCRIPTION:old notes", "DESCRIPTION:new notes");
        assert_ne!(edited, read, "the document carries the stripped text");

        let patch = MsgraphEvent::update_from_ical(edited.as_bytes(), read.as_bytes()).unwrap();

        let body = patch.body.as_option().expect("the edit reaches the patch");
        assert_eq!(body.content_type, Some(MsgraphBodyType::Text));
        assert_eq!(body.content.as_deref(), Some("new notes"));
    }

    /// A document with its description lines, folded ones included, removed.
    fn without_description(ical: &str) -> String {
        let mut kept = String::new();
        let mut dropping = false;
        for line in ical.split_inclusive("\r\n") {
            if !line.starts_with(' ') {
                dropping = line.starts_with("DESCRIPTION") || line.starts_with("X-ALT-DESC");
            }
            if !dropping {
                kept.push_str(line);
            }
        }
        kept
    }

    #[test]
    fn an_empty_html_body_matches_a_document_without_description() {
        let mut event = utc_read();
        event.body = MsgraphField::Set(MsgraphItemBody {
            content_type: Some(MsgraphBodyType::Html),
            content: Some("<html><body></body></html>".into()),
        });
        let read = event.to_ical();
        let edited = without_description(&read);

        let patch = MsgraphEvent::update_from_ical(edited.as_bytes(), read.as_bytes()).unwrap();

        assert_eq!(patch.body, MsgraphField::Unset, "no body change to send");
    }

    #[test]
    fn a_cleared_description_sends_an_empty_text_body() {
        let read = html_read();
        let edited = without_description(&read);

        let patch = MsgraphEvent::update_from_ical(edited.as_bytes(), read.as_bytes()).unwrap();

        let body = serde_json::to_value(&patch).unwrap()["body"].clone();
        assert_eq!(body["contentType"], "text", "Graph refuses a null body");
        assert_eq!(body["content"], "");
    }

    #[test]
    fn the_expand_clause_names_the_stash() {
        let quoted = format!("'{MSGRAPH_EVENT_STASH_ID}'");
        assert!(MSGRAPH_EVENT_STASH_EXPAND.contains(&quoted));
    }
}
