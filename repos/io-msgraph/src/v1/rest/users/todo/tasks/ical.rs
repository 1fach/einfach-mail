//! # iCalendar projection
//!
//! Projects a Microsoft To Do task onto a VTODO and back. Graph exposes no
//! iCalendar form of a task, so a consumer needing one synthesizes the
//! document of record from the JSON resource ([`MsgraphTodoTask::to_ical`])
//! and reads it back ([`MsgraphTodoTask::from_ical`]).
//!
//! A field is managed only where it has a well-defined iCalendar slot:
//!
//! - SUMMARY, DESCRIPTION and CATEGORIES map verbatim, an HTML body read
//!   as its text;
//! - PRIORITY 1 to 4 is high, 6 to 9 low, anything else normal, read back
//!   as 1, 9 and no PRIORITY;
//! - STATUS IN-PROCESS and COMPLETED map to theirs, CANCELLED to
//!   completed, anything else to notStarted; waitingOnOthers and deferred,
//!   which RFC 5545 lacks, ride `X-MSGRAPH-STATUS` and refine an open
//!   status only;
//! - DUE and DTSTART are dates Graph keeps as midnights: a date is written
//!   as midnight in the caller's zone, and a Graph value read as the date
//!   of the nearest midnight in it, whatever zone wrote it; COMPLETED is
//!   the UTC instant Graph holds;
//! - RRULE maps onto the recurrence, anchored on DUE (DTSTART without
//!   one) as To Do repeats on the due date, a rule Graph cannot hold being
//!   refused rather than approximated;
//! - the first DISPLAY or AUDIO alarm with an absolute trigger is the
//!   reminder;
//! - a linked resource with a URL is minted read-only as an ATTACH
//!   carrying `X-MSGRAPH-LINKED-RESOURCE`, which a write drops: Graph keeps
//!   linked resources apart from the task;
//! - everything else, the UID among it, is stashed verbatim in the
//!   single-value extended property the events projection uses
//!   ([`MSGRAPH_TODO_TASK_STASH_ID`]) and spliced back on read; Graph
//!   returns it only expanded ([`MSGRAPH_TODO_TASK_STASH_EXPAND`]), which a
//!   delta refuses.
//!
//! A managed line Graph holds coarser than written (a DUE with a time, a
//! COMPLETED, PRIORITY 5, STATUS CANCELLED) is stashed too, and read back in
//! place of the line derived from Graph while Graph's value still agrees
//! with it: a document comes back as written, and an edit made in To Do
//! still wins.

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
    prop::{IcalProp, IcalPropKind, IcalPropName, action::ACTION},
    tree::{
        codec::Codec,
        cst::{IcalCst, IcalItem},
        error::IcalParseError,
        line::IcalLine,
        param::{tzid::TZID, value::VALUE},
    },
    tzdb,
    value::{IcalValue, recur::IcalRecur, text::IcalTextList},
};
use jiff::{
    Timestamp,
    civil::{Date, DateTime, Time},
    tz::TimeZone,
};
use serde_json::{from_value, to_value};

use crate::v1::{
    field::MsgraphField,
    rest::users::{
        contacts::MsgraphSingleValueExtendedProperty,
        events::{
            MsgraphDateTimeTimeZone, MsgraphDayOfWeek, MsgraphPatternedRecurrence,
            MsgraphRecurrencePatternType,
            ical::{
                Boundary, MAX_STASH_LINE, MSGRAPH_EVENT_STASH_EXPAND, MSGRAPH_EVENT_STASH_ID,
                PRODID, Zone, body_text, component_name, importance_from_ical, push_stash,
                raw_component, raw_line, recurrence, rrule, same_text, stamp_prop, stash_lines,
                text, utc_stamp, zone,
            },
        },
        messages::{MsgraphBodyType, MsgraphImportance, MsgraphItemBody},
        todo::tasks::{MsgraphTodoTask, MsgraphTodoTaskStatus},
    },
};

/// Extended-property id under which the iCalendar remainder of a task is
/// stashed, the one events use.
pub const MSGRAPH_TODO_TASK_STASH_ID: &str = MSGRAPH_EVENT_STASH_ID;

/// The `$expand` clause reading the stash back with a task, which Graph
/// omits otherwise, on a get or a listing but not on a delta.
pub const MSGRAPH_TODO_TASK_STASH_EXPAND: &str = MSGRAPH_EVENT_STASH_EXPAND;

/// The property carrying the Graph states RFC 5545 has no STATUS for.
const GRAPH_STATUS: &str = "X-MSGRAPH-STATUS";

/// The parameter marking an ATTACH minted from a linked resource.
const LINKED_RESOURCE: &str = "X-MSGRAPH-LINKED-RESOURCE";

impl MsgraphTodoTask {
    /// Projects the task onto a fresh VCALENDAR document holding one VTODO,
    /// its dates told in `time_zone` (an IANA or Windows name, UTC when
    /// unknown).
    pub fn to_ical(&self, time_zone: &str) -> String {
        let caller = Caller::new(time_zone);
        let mut zones = BTreeSet::new();
        let vtodo = vtodo(self, &caller, &mut zones);

        let mut calendar = IcalCst::v2();
        calendar.push(IcalProp::text(IcalPropKind::ProdId, vec![], PRODID));

        let anchor = [&self.last_modified_date_time, &self.created_date_time]
            .into_iter()
            .find_map(|stamp| stamp.as_deref()?.parse::<Timestamp>().ok())
            .map_or(0, |stamp| stamp.as_second());
        for zone in &zones {
            if let Some(vtimezone) = tzdb::vtimezone(zone, anchor) {
                calendar.push_component(vtimezone);
            }
        }
        calendar.push_component(vtodo);

        String::from_utf8_lossy(&calendar.to_bytes()).into_owned()
    }

    /// Projects an iCalendar document onto a task, in full state: each
    /// managed field is Set from its first VTODO or Null when absent, while
    /// Graph-only fields stay Unset. Dates without a zone are read in
    /// `time_zone`.
    pub fn from_ical(contents: &[u8], time_zone: &str) -> Result<Self, MsgraphTodoTaskIcalError> {
        let calendar = IcalCst::parse(contents).map_err(MsgraphTodoTaskIcalError::Parse)?;
        project(find_vtodo(&calendar)?, &Caller::new(time_zone))
    }

    /// Projects an iCalendar document onto a create body: the full-state
    /// projection minus its nulls and empty collections, which a fresh task
    /// does not need.
    pub fn create_from_ical(
        contents: &[u8],
        time_zone: &str,
    ) -> Result<Self, MsgraphTodoTaskIcalError> {
        let mut task = Self::from_ical(contents, time_zone)?;

        if stash_lines(&task.single_value_extended_properties).is_empty() {
            task.single_value_extended_properties = MsgraphField::Unset;
        }

        let mut body = to_value(&task).map_err(MsgraphTodoTaskIcalError::Json)?;
        if let Some(object) = body.as_object_mut() {
            object.retain(|_, value| {
                !value.is_null() && value.as_array().is_none_or(|array| !array.is_empty())
            });
        }

        from_value(body).map_err(MsgraphTodoTaskIcalError::Json)
    }

    /// Projects an iCalendar document onto an update body: the fields that
    /// differ from the base document (the state last synced), the unchanged
    /// ones left Unset and out of the PATCH.
    pub fn update_from_ical(
        contents: &[u8],
        base: &[u8],
        time_zone: &str,
    ) -> Result<Self, MsgraphTodoTaskIcalError> {
        let mut task = Self::from_ical(contents, time_zone)?;
        task.unset_unchanged(&Self::from_ical(base, time_zone)?);
        Ok(task)
    }

    /// The iCalendar UID the stash carries, `None` for a task no document
    /// was ever written to.
    ///
    /// A sync engine checks it after a write: a server that dropped the
    /// stash would hand the task back under its Graph id.
    pub fn stashed_uid(&self) -> Option<String> {
        stash_lines(&self.single_value_extended_properties)
            .iter()
            .find_map(|raw| {
                let bytes = crlf(raw);
                let line = parse_line(&bytes)?;
                line.name
                    .get()
                    .eq_ignore_ascii_case("UID")
                    .then(|| line.raw_value_str().trim().to_string())
                    .filter(|uid| !uid.is_empty())
            })
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
            title,
            categories,
            importance,
            status,
            due_date_time,
            start_date_time,
            completed_date_time,
            is_reminder_on,
            reminder_date_time,
            recurrence,
            single_value_extended_properties,
        );
    }
}

/// An iCalendar document that cannot become a To Do task.
#[derive(Debug)]
pub enum MsgraphTodoTaskIcalError {
    /// The document does not parse.
    Parse(IcalParseError),
    /// The document holds no VTODO.
    NoTodo,
    /// The document holds a component To Do does not model.
    NotATodo(String),
    /// The RRULE has no Graph recurrence pattern.
    UnsupportedRecurrence(String),
    /// The projected task does not reshape through JSON.
    Json(serde_json::Error),
}

impl fmt::Display for MsgraphTodoTaskIcalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "Invalid iCalendar: {err}"),
            Self::NoTodo => write!(f, "The iCalendar contents carry no VTODO"),
            Self::NotATodo(name) => {
                write!(
                    f,
                    "Microsoft To Do models no {name}: its lists hold tasks only"
                )
            }
            Self::UnsupportedRecurrence(rule) => {
                write!(f, "Microsoft To Do cannot hold the recurrence `{rule}`")
            }
            Self::Json(err) => write!(f, "Cannot shape the Graph task body: {err}"),
        }
    }
}

impl core::error::Error for MsgraphTodoTaskIcalError {}

/// The zone a caller tells dates in, resolved once.
struct Caller {
    /// The IANA name Graph is sent.
    name: String,
    /// The zone itself.
    tz: TimeZone,
}

impl Caller {
    /// Resolves an IANA or Windows name, UTC when neither table knows it.
    fn new(time_zone: &str) -> Self {
        if let Zone::Iana(name) = zone(Some(time_zone))
            && let Ok(tz) = TimeZone::get(&name)
        {
            return Self { name, tz };
        }

        Self {
            name: String::from("UTC"),
            tz: TimeZone::UTC,
        }
    }

    /// The Graph value of midnight of `date` in this zone.
    fn midnight(&self, date: Date) -> MsgraphDateTimeTimeZone {
        MsgraphDateTimeTimeZone {
            date_time: graph_date_time(date.at(0, 0, 0, 0)),
            time_zone: Some(self.name.clone()),
        }
    }

    /// The date of a Graph value: that of the nearest midnight in this
    /// zone.
    ///
    /// Graph keeps due, start and completion dates as a midnight in the
    /// zone that wrote them, so this is the written date whenever that zone
    /// is within twelve hours of this one.
    fn date_of(&self, value: &MsgraphDateTimeTimeZone) -> Option<Date> {
        let local = instant(value)?.to_zoned(self.tz.clone()).datetime();

        if local.time() >= Time::constant(12, 0, 0, 0) {
            local.date().tomorrow().ok()
        } else {
            Some(local.date())
        }
    }

    /// The Graph value a DUE or DTSTART line is sent as: a date as midnight
    /// in this zone, a UTC or zoned wall time as it is, a floating one in
    /// this zone. Graph keeps the midnight of its date.
    fn date_slot(&self, line: &IcalLine<'_>) -> Option<MsgraphDateTimeTimeZone> {
        let value = line.raw_value_str();
        let value = value.trim();

        if is_date(line, value) {
            return Some(self.midnight(Date::strptime("%Y%m%d", value).ok()?));
        }

        let (local, zone) = match value.strip_suffix('Z') {
            Some(local) => (local, String::from("UTC")),
            None => match line.param::<TZID>() {
                Some(tzid) => (value, tzid.into_owned()),
                None => (value, self.name.clone()),
            },
        };

        Some(MsgraphDateTimeTimeZone {
            date_time: graph_date_time(DateTime::strptime("%Y%m%dT%H%M%S", local).ok()?),
            time_zone: Some(zone),
        })
    }

    /// The Graph value a COMPLETED line is sent as: its instant told in this
    /// zone, so Graph keeps the midnight of the date it falls on here.
    fn completed_slot(&self, line: &IcalLine<'_>) -> Option<MsgraphDateTimeTimeZone> {
        let instant = instant(&self.date_slot(line)?)?;
        let local = instant.to_zoned(self.tz.clone()).datetime();

        Some(MsgraphDateTimeTimeZone {
            date_time: graph_date_time(local),
            time_zone: Some(self.name.clone()),
        })
    }
}

/// Projects one VTODO onto a task, in full state.
fn project(
    vtodo: &IcalCst<'_>,
    caller: &Caller,
) -> Result<MsgraphTodoTask, MsgraphTodoTaskIcalError> {
    let mut task = MsgraphTodoTask {
        title: MsgraphField::Null,
        categories: MsgraphField::Set(Vec::new()),
        importance: MsgraphField::Set(MsgraphImportance::Normal),
        status: MsgraphField::Set(MsgraphTodoTaskStatus::NotStarted),
        due_date_time: MsgraphField::Null,
        start_date_time: MsgraphField::Null,
        completed_date_time: MsgraphField::Null,
        is_reminder_on: MsgraphField::Set(false),
        reminder_date_time: MsgraphField::Null,
        recurrence: MsgraphField::Null,
        ..Default::default()
    };

    let mut description = None;
    let mut graph_status = None;
    let mut rrule_line = None;
    let mut anchors = (None, None);
    let mut stash = Vec::new();

    for item in &vtodo.items {
        match item {
            IcalItem::Prop(line) => {
                let name = line.name.get();
                let value = line.raw_value_str();
                let value = value.trim();

                let consumed = if name.eq_ignore_ascii_case(GRAPH_STATUS) {
                    graph_status = graph_status_from_ical(value);
                    true
                } else if is_linked_resource(line) {
                    true
                } else {
                    match name.parse::<IcalPropKind>() {
                        Ok(IcalPropKind::Summary) => {
                            task.title = MsgraphField::Set(text(line));
                            true
                        }
                        Ok(IcalPropKind::Description) => {
                            description = Some(text(line));
                            true
                        }
                        Ok(IcalPropKind::Categories) => {
                            let list = IcalTextList::decode(&line.value);
                            if let MsgraphField::Set(categories) = &mut task.categories {
                                categories.extend(list.0.into_iter().map(Cow::into_owned));
                            }
                            true
                        }
                        Ok(IcalPropKind::Priority) => {
                            task.importance = MsgraphField::Set(importance_of(value));
                            true
                        }
                        Ok(IcalPropKind::Status) => {
                            task.status = MsgraphField::Set(status_of(value));
                            true
                        }
                        Ok(IcalPropKind::Due) => {
                            let slot = caller.date_slot(line);
                            anchors.0 = slot.as_ref().and_then(|slot| anchor(slot, line));
                            task.due_date_time = MsgraphField::set_or_null(slot);
                            true
                        }
                        Ok(IcalPropKind::DtStart) => {
                            let slot = caller.date_slot(line);
                            anchors.1 = slot.as_ref().and_then(|slot| anchor(slot, line));
                            task.start_date_time = MsgraphField::set_or_null(slot);
                            true
                        }
                        Ok(IcalPropKind::Completed) => {
                            task.completed_date_time =
                                MsgraphField::set_or_null(caller.completed_slot(line));
                            true
                        }
                        Ok(IcalPropKind::RRule) => {
                            rrule_line = Some(line);
                            continue;
                        }
                        // NOTE: Graph sets the stamps itself, so an incoming
                        // value is consumed rather than stashed.
                        Ok(
                            IcalPropKind::DtStamp
                            | IcalPropKind::Created
                            | IcalPropKind::LastModified
                            | IcalPropKind::Sequence,
                        ) => true,
                        _ => false,
                    }
                };

                if !consumed || needs_shadow(line) {
                    let raw = raw_line(line);
                    if raw.len() <= MAX_STASH_LINE {
                        stash.push(raw);
                    }
                }
            }
            IcalItem::Component(alarm) => match reminder(alarm) {
                Some(at) if task.reminder_date_time.as_option().is_none() => {
                    task.is_reminder_on = MsgraphField::Set(true);
                    task.reminder_date_time = MsgraphField::Set(at);
                }
                _ => stash.extend(raw_component(alarm)),
            },
            IcalItem::Opaque(bytes) => stash.push(String::from_utf8_lossy(bytes).into_owned()),
        }
    }

    if task.status.as_option() == Some(&MsgraphTodoTaskStatus::NotStarted)
        && let Some(status) = graph_status
    {
        task.status = MsgraphField::Set(status);
    }

    if let Some(line) = rrule_line {
        let rule = IcalRecur::decode(&line.value).0.trim().to_owned();
        match anchors.0.or(anchors.1) {
            Some(anchor) => {
                let recurrence = task_recurrence(&rule, &anchor)
                    .ok_or_else(|| MsgraphTodoTaskIcalError::UnsupportedRecurrence(rule.clone()))?;
                // NOTE: read back, a rule is spelled by the projection; one
                // spelled otherwise keeps its written form in the stash.
                if !line.params.is_empty() || rrule(&recurrence, None, true) != Some(rule) {
                    stash.push(raw_line(line));
                }
                task.recurrence = MsgraphField::Set(recurrence);
            }
            None => stash.push(raw_line(line)),
        }
    }

    task.body = MsgraphField::Set(MsgraphItemBody {
        content_type: Some(MsgraphBodyType::Text),
        content: Some(description.unwrap_or_default()),
    });

    // NOTE: the stash entry is always Set (empty when nothing is left),
    // so an update can tell a cleared stash from an unchanged one.
    task.single_value_extended_properties =
        MsgraphField::Set(vec![MsgraphSingleValueExtendedProperty {
            id: MSGRAPH_TODO_TASK_STASH_ID.to_string(),
            value: stash.join("\n"),
        }]);

    Ok(task)
}

/// Builds the VTODO of a task.
fn vtodo(
    task: &MsgraphTodoTask,
    caller: &Caller,
    zones: &mut BTreeSet<String>,
) -> IcalCst<'static> {
    let stash = Stash::of(task);

    let mut vtodo = IcalCst::empty("VTODO");
    let uid = task.stashed_uid().unwrap_or_else(|| task.id.clone());
    vtodo.push(IcalProp::text(IcalPropKind::Uid, vec![], uid));

    // NOTE: DTSTAMP is mandatory (RFC 5545 3.6.2) and Graph keeps no field
    // of its own for it, so the last modification stands in.
    let stamp = task
        .last_modified_date_time
        .as_deref()
        .or(task.created_date_time.as_deref())
        .and_then(utc_stamp);
    if let Some(stamp) = stamp {
        vtodo.push(stamp_prop(IcalPropKind::DtStamp, stamp));
    }

    if let Some(title) = task.title.as_option() {
        vtodo.push(IcalProp::text(IcalPropKind::Summary, vec![], title.clone()));
    }

    let description = body_text(&task.body);
    if !description.is_empty() {
        vtodo.push(IcalProp::text(
            IcalPropKind::Description,
            vec![],
            description,
        ));
    }

    let due = task.due_date_time.as_option();
    let graph_anchor = due
        .and_then(|due| caller.date_of(due))
        .map(|date| Boundary {
            local: date.at(0, 0, 0, 0),
            all_day: true,
            zone: String::from("UTC"),
        });

    for kind in [
        IcalPropKind::DtStart,
        IcalPropKind::Due,
        IcalPropKind::Completed,
        IcalPropKind::Status,
        IcalPropKind::Priority,
        IcalPropKind::RRule,
    ] {
        if let Some(shadow) = stash.shadow(kind) {
            let bytes = crlf(shadow);
            let line = parse_line(&bytes);
            if line
                .as_ref()
                .is_some_and(|line| agrees(task, line, caller, graph_anchor.as_ref()))
            {
                if let Some(tzid) = line.as_ref().and_then(|line| line.param::<TZID>()) {
                    zones.insert(tzid.into_owned());
                }
                let _ = vtodo.push_raw(shadow);
                continue;
            }
        }

        if let Some(prop) = derived(task, kind, caller) {
            vtodo.push(prop);
        }
    }

    if let Some(status) = task
        .status
        .as_option()
        .and_then(|status| graph_status_to_ical(*status))
    {
        vtodo.push(IcalProp::text(GRAPH_STATUS, vec![], status));
    }

    let categories: Vec<Cow<'static, str>> = task
        .categories
        .as_option()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter(|category| !category.is_empty())
        .map(|category| Cow::Owned(category.clone()))
        .collect();
    if !categories.is_empty() {
        vtodo.push(IcalProp {
            name: IcalPropName::Kind(IcalPropKind::Categories),
            params: Vec::new(),
            value: IcalValue::TextList(IcalTextList(categories)),
        });
    }

    for (kind, stamp) in [
        (IcalPropKind::Created, &task.created_date_time),
        (IcalPropKind::LastModified, &task.last_modified_date_time),
    ] {
        if let Some(stamp) = stamp.as_deref().and_then(utc_stamp) {
            vtodo.push(stamp_prop(kind, stamp));
        }
    }

    for resource in &task.linked_resources {
        let Some(url) = resource.web_url.as_deref().filter(|url| !url.is_empty()) else {
            continue;
        };
        let id = resource.id.replace('"', "");
        let _ = vtodo.push_raw(&format!("ATTACH;{LINKED_RESOURCE}=\"{id}\":{url}"));
    }

    push_stash(&mut vtodo, &stash.rest);

    if task.is_reminder_on.as_option() == Some(&true)
        && let Some(stamp) = task
            .reminder_date_time
            .as_option()
            .and_then(instant)
            .map(|at| at.strftime("%Y%m%dT%H%M%SZ").to_string())
    {
        let mut alarm = IcalCst::empty("VALARM");
        alarm.push(IcalProp::text(IcalPropKind::Action, vec![], "DISPLAY"));
        alarm.push(IcalProp {
            name: IcalPropName::Kind(IcalPropKind::Trigger),
            params: vec![IcalParam::Value("DATE-TIME".into())],
            value: IcalValue::DateTime(stamp.into()),
        });
        alarm.push(IcalProp::text(
            IcalPropKind::Description,
            vec![],
            "Reminder",
        ));
        vtodo.push_component(alarm);
    }

    vtodo
}

/// The stashed lines of a task, sorted for the read.
struct Stash {
    /// The first written form of each managed line, by name.
    shadows: Vec<(String, String)>,
    /// Every other line, restored verbatim.
    rest: Vec<String>,
}

impl Stash {
    /// Sorts the stash of `task`: top-level lines naming a managed property
    /// are shadows, the UID is written apart, minted lines are dropped.
    fn of(task: &MsgraphTodoTask) -> Self {
        let mut shadows: Vec<(String, String)> = Vec::new();
        let mut rest = Vec::new();
        let mut depth = 0_usize;

        for raw in stash_lines(&task.single_value_extended_properties) {
            let bytes = crlf(&raw);
            let Some(line) = parse_line(&bytes) else {
                continue;
            };
            let name = line.name.get().to_ascii_uppercase();

            if depth == 0 {
                let managed = matches!(
                    name.as_str(),
                    "DTSTART" | "DUE" | "COMPLETED" | "STATUS" | "PRIORITY" | "RRULE"
                );
                if managed {
                    if !shadows.iter().any(|(shadow, _)| *shadow == name) {
                        shadows.push((name, raw));
                    }
                    continue;
                }
                if name == "UID" || name == GRAPH_STATUS || is_linked_resource(&line) {
                    continue;
                }
            }

            match name.as_str() {
                "BEGIN" => depth += 1,
                "END" => depth = depth.saturating_sub(1),
                _ => {}
            }
            rest.push(raw);
        }

        Self { shadows, rest }
    }

    /// The written form of the managed line `kind`, if stashed.
    fn shadow(&self, kind: IcalPropKind) -> Option<&str> {
        let name = IcalPropName::Kind(kind);
        self.shadows
            .iter()
            .find(|(shadow, _)| shadow.eq_ignore_ascii_case(&name))
            .map(|(_, raw)| raw.as_str())
    }
}

/// The line Graph's value of `kind` reads back as, `None` when it has
/// none.
fn derived(
    task: &MsgraphTodoTask,
    kind: IcalPropKind,
    caller: &Caller,
) -> Option<IcalProp<'static>> {
    match kind {
        IcalPropKind::DtStart | IcalPropKind::Due => {
            let value = match kind {
                IcalPropKind::Due => task.due_date_time.as_option()?,
                _ => task.start_date_time.as_option()?,
            };
            let date = caller.date_of(value)?;
            Some(IcalProp {
                name: IcalPropName::Kind(kind),
                params: vec![IcalParam::Value("DATE".into())],
                value: IcalValue::Date(date.strftime("%Y%m%d").to_string().into()),
            })
        }
        IcalPropKind::Completed => {
            let at = instant(task.completed_date_time.as_option()?)?;
            Some(stamp_prop(kind, at.strftime("%Y%m%dT%H%M%SZ").to_string()))
        }
        IcalPropKind::Status => {
            let status = status_to_ical(*task.status.as_option()?)?;
            Some(IcalProp::text(kind, vec![], status))
        }
        IcalPropKind::Priority => {
            let priority = match task.importance.as_option()? {
                MsgraphImportance::High => "1",
                MsgraphImportance::Low => "9",
                MsgraphImportance::Normal => return None,
            };
            Some(IcalProp {
                name: IcalPropName::Kind(kind),
                params: Vec::new(),
                value: IcalValue::Integer(priority.into()),
            })
        }
        IcalPropKind::RRule => {
            let rule = rrule(task.recurrence.as_option()?, None, true)?;
            Some(IcalProp {
                name: IcalPropName::Kind(kind),
                params: Vec::new(),
                value: IcalValue::Recur(rule.into()),
            })
        }
        _ => None,
    }
}

/// Whether the written form of a managed line still says what Graph holds,
/// so it reads back in place of the derived line.
fn agrees(
    task: &MsgraphTodoTask,
    line: &IcalLine<'_>,
    caller: &Caller,
    graph_anchor: Option<&Boundary>,
) -> bool {
    let value = line.raw_value_str();
    let value = value.trim();
    let slot_date = |slot: Option<MsgraphDateTimeTimeZone>| {
        slot.and_then(|slot| slot.date_time.get(..10)?.parse::<Date>().ok())
    };
    let graph_date = |field: &MsgraphField<MsgraphDateTimeTimeZone>| {
        field.as_option().and_then(|v| caller.date_of(v))
    };

    match line.name.get().parse::<IcalPropKind>() {
        Ok(IcalPropKind::Due) => {
            slot_date(caller.date_slot(line)) == graph_date(&task.due_date_time)
        }
        Ok(IcalPropKind::DtStart) => {
            slot_date(caller.date_slot(line)) == graph_date(&task.start_date_time)
        }
        Ok(IcalPropKind::Completed) => {
            slot_date(caller.completed_slot(line)) == graph_date(&task.completed_date_time)
        }
        Ok(IcalPropKind::Priority) => {
            importance_of(value)
                == task
                    .importance
                    .as_option()
                    .copied()
                    .unwrap_or(MsgraphImportance::Normal)
        }
        Ok(IcalPropKind::Status) => {
            let graph = task.status.as_option().copied();
            status_to_ical(status_of(value)) == graph.and_then(status_to_ical)
        }
        Ok(IcalPropKind::RRule) => {
            // NOTE: a rule written without a date to anchor it never
            // reached Graph.
            let undated = task.start_date_time.as_option().is_none();
            let Some(anchor) = graph_anchor else {
                return undated && task.recurrence.as_option().is_none();
            };
            let rule = IcalRecur::decode(&line.value).0.trim().to_owned();
            let written = task_recurrence(&rule, anchor);
            let written = written
                .as_ref()
                .and_then(|written| rrule(written, None, true));
            let graph = task
                .recurrence
                .as_option()
                .and_then(|graph| rrule(graph, None, true));
            written.is_some() && written == graph
        }
        _ => false,
    }
}

/// Whether a managed line is written in a form the derived line would not
/// give back, so it is stashed beside its projection.
///
/// An RRULE is checked once its recurrence is known.
fn needs_shadow(line: &IcalLine<'_>) -> bool {
    let raw = raw_line(line);
    let value = line.raw_value_str();
    let value = value.trim();

    match line.name.get().parse::<IcalPropKind>() {
        Ok(kind @ (IcalPropKind::Due | IcalPropKind::DtStart)) => {
            let name = IcalPropName::Kind(kind);
            let date = format!("{};VALUE=DATE:{value}", &*name);
            !(is_date(line, value) && raw == date)
        }
        Ok(IcalPropKind::Completed) => true,
        Ok(IcalPropKind::Priority) => raw != "PRIORITY:1" && raw != "PRIORITY:9",
        Ok(IcalPropKind::Status) => raw != "STATUS:IN-PROCESS" && raw != "STATUS:COMPLETED",
        _ => false,
    }
}

/// The Graph recurrence of an RRULE anchored on `anchor`, `None` when Graph
/// has no pattern for it.
///
/// A weekly rule without WKST starts its weeks on Monday (RFC 5545
/// 3.3.10), which Graph would otherwise take as Sunday.
fn task_recurrence(rule: &str, anchor: &Boundary) -> Option<MsgraphPatternedRecurrence> {
    let mut recurrence = recurrence(rule, anchor).ok()?;
    let pattern = &mut recurrence.pattern;
    if pattern.pattern_type == Some(MsgraphRecurrencePatternType::Weekly)
        && pattern.first_day_of_week.is_none()
    {
        pattern.first_day_of_week = Some(MsgraphDayOfWeek::Monday);
    }
    Some(recurrence)
}

/// The recurrence anchor of a DUE or DTSTART sent as `slot`.
fn anchor(slot: &MsgraphDateTimeTimeZone, line: &IcalLine<'_>) -> Option<Boundary> {
    let value = line.raw_value_str();
    Some(Boundary {
        local: slot.date_time.parse::<DateTime>().ok()?,
        all_day: is_date(line, value.trim()),
        zone: slot
            .time_zone
            .clone()
            .unwrap_or_else(|| String::from("UTC")),
    })
}

/// The reminder a DISPLAY or AUDIO alarm with an absolute trigger stands
/// for, `None` for any other alarm.
fn reminder(alarm: &IcalCst<'_>) -> Option<MsgraphDateTimeTimeZone> {
    if component_name(alarm) != "VALARM" {
        return None;
    }
    let action = alarm.prop::<ACTION>()?;
    if !matches!(action.0.trim().to_uppercase().as_str(), "DISPLAY" | "AUDIO") {
        return None;
    }

    let trigger = alarm.items.iter().find_map(|item| match item {
        IcalItem::Prop(line) if line.name.get().eq_ignore_ascii_case("TRIGGER") => Some(line),
        _ => None,
    })?;
    if !trigger
        .param::<VALUE>()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("DATE-TIME"))
    {
        return None;
    }

    let value = trigger.raw_value_str();
    let utc = DateTime::strptime("%Y%m%dT%H%M%S", value.trim().strip_suffix('Z')?).ok()?;

    Some(MsgraphDateTimeTimeZone {
        date_time: graph_date_time(utc),
        time_zone: Some(String::from("UTC")),
    })
}

/// The first VTODO of a document.
fn find_vtodo<'a>(calendar: &'a IcalCst<'a>) -> Result<&'a IcalCst<'a>, MsgraphTodoTaskIcalError> {
    let components = || {
        calendar.items.iter().filter_map(|item| match item {
            IcalItem::Component(component) => Some(component.as_ref()),
            _ => None,
        })
    };

    if let Some(vtodo) = components().find(|component| component_name(component) == "VTODO") {
        return Ok(vtodo);
    }

    match components()
        .map(component_name)
        .find(|name| name != "VTIMEZONE")
    {
        Some(name) => Err(MsgraphTodoTaskIcalError::NotATodo(name)),
        None => Err(MsgraphTodoTaskIcalError::NoTodo),
    }
}

/// The instant of a Graph wall time in a zone, a zone neither table knows
/// read as UTC.
fn instant(value: &MsgraphDateTimeTimeZone) -> Option<Timestamp> {
    let local = value
        .date_time
        .split('.')
        .next()?
        .parse::<DateTime>()
        .ok()?;
    let tz = match zone(value.time_zone.as_deref()) {
        Zone::Iana(name) => TimeZone::get(&name).unwrap_or(TimeZone::UTC),
        Zone::Utc | Zone::Unknown(_) => TimeZone::UTC,
    };
    Some(local.to_zoned(tz).ok()?.timestamp())
}

/// A wall time in Graph's form, `YYYY-MM-DDTHH:MM:SS`.
fn graph_date_time(local: DateTime) -> String {
    local.strftime("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Whether a DUE, DTSTART or COMPLETED value is a date.
fn is_date(line: &IcalLine<'_>, value: &str) -> bool {
    line.param::<VALUE>()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("DATE"))
        || (value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Whether a line is an ATTACH minted from a linked resource.
fn is_linked_resource(line: &IcalLine<'_>) -> bool {
    line.name.get().eq_ignore_ascii_case("ATTACH")
        && line
            .params
            .iter()
            .any(|param| param.name.get().eq_ignore_ascii_case(LINKED_RESOURCE))
}

/// A stashed line ended for tokenising.
fn crlf(raw: &str) -> String {
    let mut bytes = String::with_capacity(raw.len() + 2);
    bytes.push_str(raw);
    bytes.push_str("\r\n");
    bytes
}

/// One stashed logical line, tokenised from [`crlf`].
fn parse_line(bytes: &str) -> Option<IcalLine<'_>> {
    let (line, _) = IcalLine::take(bytes.as_bytes()).ok()?;
    Some(line)
}

/// The importance of a PRIORITY, normal for 0, 5 or garbage.
fn importance_of(value: &str) -> MsgraphImportance {
    importance_from_ical(value).unwrap_or(MsgraphImportance::Normal)
}

/// The Graph status of a STATUS.
fn status_of(value: &str) -> MsgraphTodoTaskStatus {
    match value.to_ascii_uppercase().as_str() {
        "IN-PROCESS" => MsgraphTodoTaskStatus::InProgress,
        "COMPLETED" | "CANCELLED" => MsgraphTodoTaskStatus::Completed,
        _ => MsgraphTodoTaskStatus::NotStarted,
    }
}

/// The STATUS of a Graph status, `None` for the open ones.
fn status_to_ical(status: MsgraphTodoTaskStatus) -> Option<&'static str> {
    match status {
        MsgraphTodoTaskStatus::InProgress => Some("IN-PROCESS"),
        MsgraphTodoTaskStatus::Completed => Some("COMPLETED"),
        _ => None,
    }
}

/// The `X-MSGRAPH-STATUS` of a Graph status RFC 5545 has no STATUS for.
fn graph_status_to_ical(status: MsgraphTodoTaskStatus) -> Option<&'static str> {
    match status {
        MsgraphTodoTaskStatus::WaitingOnOthers => Some("waitingOnOthers"),
        MsgraphTodoTaskStatus::Deferred => Some("deferred"),
        _ => None,
    }
}

/// The Graph status of an `X-MSGRAPH-STATUS`.
fn graph_status_from_ical(value: &str) -> Option<MsgraphTodoTaskStatus> {
    match value.to_ascii_lowercase().as_str() {
        "waitingonothers" => Some(MsgraphTodoTaskStatus::WaitingOnOthers),
        "deferred" => Some(MsgraphTodoTaskStatus::Deferred),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use alloc::{string::String, vec::Vec};

    use crate::v1::{
        field::MsgraphField,
        rest::users::{
            events::{
                MsgraphDateTimeTimeZone, MsgraphDayOfWeek, MsgraphRecurrencePatternType,
                MsgraphRecurrenceRangeType,
            },
            messages::MsgraphImportance,
            todo::tasks::{MsgraphTodoTask, MsgraphTodoTaskStatus, ical::*},
        },
    };

    const PARIS: &str = "Europe/Paris";

    /// A task as Graph answers it, every date in UTC.
    fn graph_task() -> MsgraphTodoTask {
        serde_json::from_str(
            r#"{
                "@odata.etag": "W/\"vpggFQef0kKogJ7hQ62IOgAAFiCgrQ==\"",
                "importance": "high",
                "isReminderOn": true,
                "status": "waitingOnOthers",
                "title": "Call the plumber",
                "createdDateTime": "2026-11-06T07:48:08.8504724Z",
                "lastModifiedDateTime": "2026-11-06T07:48:09.3102687Z",
                "hasAttachments": false,
                "categories": ["inbox", "@phone"],
                "id": "AAMkTASK=",
                "body": {"content": "line one\nline two", "contentType": "text"},
                "dueDateTime": {"dateTime": "2026-11-19T23:00:00.0000000", "timeZone": "UTC"},
                "startDateTime": {"dateTime": "2026-11-17T23:00:00.0000000", "timeZone": "UTC"},
                "reminderDateTime": {"dateTime": "2026-11-20T08:00:00.0000000", "timeZone": "UTC"},
                "linkedResources": [{
                    "webUrl": "https://outlook.office365.com/owa/?ItemID=AAMk",
                    "applicationName": "Outlook",
                    "displayName": "Leak in the kitchen",
                    "id": "ba124565-3cff-4dce-96c6-544a92e4579b"
                }],
                "singleValueExtendedProperties": [{
                    "id": "String {c8e5e5cf-3f6c-4f0a-9d4e-52f1e7b2a9d3} Name pimalaya-ical",
                    "value": "UID:task-1\nCONTACT:Bob"
                }]
            }"#,
        )
        .unwrap()
    }

    /// The VTODO lines of a document, stamps aside, in order.
    fn lines(ical: &str) -> Vec<&str> {
        let start = ical.find("BEGIN:VTODO").unwrap();
        let end = ical.find("END:VTODO").unwrap();
        ical[start..end]
            .split("\r\n")
            .filter(|line| !line.is_empty())
            .filter(|line| {
                !["DTSTAMP", "CREATED", "LAST-MODIFIED"]
                    .iter()
                    .any(|n| line.starts_with(n))
            })
            .collect()
    }

    /// The task Graph keeps from a create body: dates truncated to the
    /// midnight of the zone they were sent in, answered in UTC, and the
    /// stash returned only through the expand.
    fn stored(mut task: MsgraphTodoTask) -> MsgraphTodoTask {
        for field in [
            &mut task.due_date_time,
            &mut task.start_date_time,
            &mut task.completed_date_time,
        ] {
            if let MsgraphField::Set(value) = field {
                let date = value.date_time.get(..10).unwrap().parse::<Date>().unwrap();
                let tz = TimeZone::get(value.time_zone.as_deref().unwrap()).unwrap();
                let utc = date.at(0, 0, 0, 0).to_zoned(tz).unwrap().timestamp();
                *value = MsgraphDateTimeTimeZone {
                    date_time: utc.strftime("%Y-%m-%dT%H:%M:%S.0000000").to_string(),
                    time_zone: Some("UTC".into()),
                };
            }
        }
        task.id = "AAMkTASK=".into();
        task.last_modified_date_time = Some("2026-11-06T08:00:00Z".into());
        task
    }

    #[test]
    fn a_graph_task_reads_as_one_vtodo() {
        let ical = graph_task().to_ical(PARIS);

        assert_eq!(
            lines(&ical),
            [
                "BEGIN:VTODO",
                "UID:task-1",
                "SUMMARY:Call the plumber",
                "DESCRIPTION:line one\\nline two",
                "DTSTART;VALUE=DATE:20261118",
                "DUE;VALUE=DATE:20261120",
                "PRIORITY:1",
                "X-MSGRAPH-STATUS:waitingOnOthers",
                "CATEGORIES:inbox,@phone",
                "ATTACH;X-MSGRAPH-LINKED-RESOURCE=\"ba124565-3cff-4dce-96c6-544a92e4579b\":https://outlook.office365.com/owa/?ItemID=AAMk",
                "CONTACT:Bob",
                "BEGIN:VALARM",
                "ACTION:DISPLAY",
                "TRIGGER;VALUE=DATE-TIME:20261120T080000Z",
                "DESCRIPTION:Reminder",
                "END:VALARM",
            ]
        );
    }

    #[test]
    fn a_due_date_is_the_nearest_midnight_in_the_callers_zone() {
        let mut task = MsgraphTodoTask::default();

        // NOTE: To Do keeps the midnight of the zone that wrote the date.
        for (stored, zone, date) in [
            ("2026-11-20T00:00:00.0000000", "UTC", "20261120"),
            ("2026-11-20T00:00:00.0000000", "Europe/Paris", "20261120"),
            ("2026-11-19T23:00:00.0000000", "Europe/Paris", "20261120"),
            (
                "2026-11-20T05:00:00.0000000",
                "America/New_York",
                "20261120",
            ),
            (
                "2026-11-20T00:00:00.0000000",
                "Pacific Standard Time",
                "20261120",
            ),
        ] {
            task.due_date_time = MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: stored.into(),
                time_zone: Some("UTC".into()),
            });
            let expected = format!("DUE;VALUE=DATE:{date}");
            assert!(
                lines(&task.to_ical(zone)).contains(&expected.as_str()),
                "{zone}"
            );
        }
    }

    #[test]
    fn a_written_task_reads_back_as_written() {
        let written = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Pimalaya//android//EN\r\n\
            BEGIN:VTODO\r\nUID:task-2\r\nDTSTAMP:20261106T080000Z\r\n\
            SUMMARY:Answer Léa\r\nDUE;VALUE=DATE:20261030\r\nPRIORITY:5\r\n\
            STATUS:NEEDS-ACTION\r\nCATEGORIES:Work,calls\r\n\
            CONTACT;ALTREP=\"mailto:lea@example.net\":Léa Martin\r\n\
            RELATED-TO;RELTYPE=PARENT:project-1\r\nATTACH:mid:abc%2F1@example.net\r\n\
            X-APP-ORDER:3\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let body = MsgraphTodoTask::create_from_ical(written.as_bytes(), PARIS).unwrap();
        assert_eq!(
            body.importance,
            MsgraphField::Set(MsgraphImportance::Normal)
        );
        assert_eq!(
            body.status,
            MsgraphField::Set(MsgraphTodoTaskStatus::NotStarted)
        );
        assert_eq!(
            body.categories,
            MsgraphField::Set(vec!["Work".into(), "calls".into()])
        );
        assert_eq!(
            body.due_date_time,
            MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: "2026-10-30T00:00:00".into(),
                time_zone: Some(PARIS.into()),
            })
        );

        let read = stored(body).to_ical(PARIS);
        let mut expected = lines(written);
        let mut read = lines(&read);
        expected.sort_unstable();
        read.sort_unstable();
        assert_eq!(read, expected);
    }

    #[test]
    fn coarse_lines_come_back_while_graph_agrees() {
        let written = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:task-3\r\n\
            DUE;TZID=Europe/Paris:20261120T143000\r\nCOMPLETED:20261105T231500Z\r\n\
            STATUS:CANCELLED\r\nPRIORITY:3\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let body = MsgraphTodoTask::create_from_ical(written.as_bytes(), PARIS).unwrap();
        assert_eq!(
            body.status,
            MsgraphField::Set(MsgraphTodoTaskStatus::Completed)
        );
        assert_eq!(body.importance, MsgraphField::Set(MsgraphImportance::High));
        assert_eq!(
            body.completed_date_time,
            MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: "2026-11-06T00:15:00".into(),
                time_zone: Some(PARIS.into()),
            })
        );

        let task = stored(body);
        let read = task.to_ical(PARIS);
        for line in [
            "DUE;TZID=Europe/Paris:20261120T143000",
            "COMPLETED:20261105T231500Z",
            "STATUS:CANCELLED",
            "PRIORITY:3",
        ] {
            assert!(lines(&read).contains(&line), "{line}");
        }
        assert!(read.contains("BEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\n"));

        let mut edited = task;
        edited.importance = MsgraphField::Set(MsgraphImportance::Low);
        edited.status = MsgraphField::Set(MsgraphTodoTaskStatus::InProgress);
        edited.due_date_time = MsgraphField::Set(MsgraphDateTimeTimeZone {
            date_time: "2026-11-21T23:00:00.0000000".into(),
            time_zone: Some("UTC".into()),
        });
        let read = edited.to_ical(PARIS);
        let read = lines(&read);
        for line in ["DUE;VALUE=DATE:20261122", "STATUS:IN-PROCESS", "PRIORITY:9"] {
            assert!(read.contains(&line), "{line}");
        }
        assert!(
            !read
                .iter()
                .any(|line| line.contains("CANCELLED") || *line == "PRIORITY:3")
        );
    }

    #[test]
    fn graph_only_states_ride_their_own_property() {
        let open = |doc: &str| {
            let doc = format!(
                "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:4\r\n{doc}END:VTODO\r\nEND:VCALENDAR\r\n"
            );
            MsgraphTodoTask::from_ical(doc.as_bytes(), PARIS)
                .unwrap()
                .status
        };

        assert_eq!(
            open("X-MSGRAPH-STATUS:waitingOnOthers\r\n"),
            MsgraphField::Set(MsgraphTodoTaskStatus::WaitingOnOthers)
        );
        assert_eq!(
            open("STATUS:NEEDS-ACTION\r\nX-MSGRAPH-STATUS:deferred\r\n"),
            MsgraphField::Set(MsgraphTodoTaskStatus::Deferred)
        );
        assert_eq!(
            open("STATUS:COMPLETED\r\nX-MSGRAPH-STATUS:deferred\r\n"),
            MsgraphField::Set(MsgraphTodoTaskStatus::Completed)
        );

        let mut task = graph_task();
        task.status = MsgraphField::Set(MsgraphTodoTaskStatus::Deferred);
        let read = task.to_ical(PARIS);
        assert!(lines(&read).contains(&"X-MSGRAPH-STATUS:deferred"));
        assert!(!read.contains("\r\nSTATUS:"));
    }

    #[test]
    fn minted_attachments_are_not_written_back() {
        let read = graph_task().to_ical(PARIS);
        let task = MsgraphTodoTask::from_ical(read.as_bytes(), PARIS).unwrap();

        let stash = stash_lines(&task.single_value_extended_properties);
        assert_eq!(stash, ["UID:task-1", "CONTACT:Bob"]);
        assert!(task.linked_resources.is_empty());
    }

    #[test]
    fn an_update_sends_what_changed() {
        let base = graph_task().to_ical(PARIS);
        let edited = base.replace("SUMMARY:Call the plumber", "SUMMARY:Call the plumber again");

        let update =
            MsgraphTodoTask::update_from_ical(edited.as_bytes(), base.as_bytes(), PARIS).unwrap();

        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            serde_json::json!({"title": "Call the plumber again"})
        );
    }

    #[test]
    fn a_recurrence_repeats_on_the_due_date() {
        let doc = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:5\r\nDUE;VALUE=DATE:20261109\r\n\
            RRULE:FREQ=WEEKLY;INTERVAL=2;COUNT=3\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let task = MsgraphTodoTask::from_ical(doc.as_bytes(), PARIS).unwrap();
        let recurrence = task.recurrence.as_option().unwrap();
        assert_eq!(
            recurrence.pattern.pattern_type,
            Some(MsgraphRecurrencePatternType::Weekly)
        );
        assert_eq!(recurrence.pattern.days_of_week, [MsgraphDayOfWeek::Monday]);
        assert_eq!(
            recurrence.range.range_type,
            Some(MsgraphRecurrenceRangeType::Numbered)
        );
        assert_eq!(recurrence.range.start_date.as_deref(), Some("2026-11-09"));
        assert_eq!(
            recurrence.pattern.first_day_of_week,
            Some(MsgraphDayOfWeek::Monday)
        );

        // NOTE: how Graph answers that recurrence, padded.
        let graph = |first_day: &str| -> MsgraphTodoTask {
            let json = r#"{
                "id": "AAMkREC=",
                "dueDateTime": {"dateTime": "2026-11-09T00:00:00.0000000", "timeZone": "Europe/Paris"},
                "recurrence": {
                    "pattern": {"type": "weekly", "interval": 2, "month": 0, "dayOfMonth": 0,
                        "daysOfWeek": ["monday"], "firstDayOfWeek": "FIRST", "index": "first"},
                    "range": {"type": "numbered", "startDate": "2026-11-09", "endDate": "2026-12-07",
                        "recurrenceTimeZone": "Europe/Paris", "numberOfOccurrences": 3}
                }
            }"#;
            serde_json::from_str(&json.replace("FIRST", first_day)).unwrap()
        };

        let read = MsgraphTodoTask {
            single_value_extended_properties: task.single_value_extended_properties.clone(),
            ..graph("monday")
        }
        .to_ical(PARIS);
        assert!(lines(&read).contains(&"RRULE:FREQ=WEEKLY;INTERVAL=2;COUNT=3"));

        let read = MsgraphTodoTask {
            single_value_extended_properties: task.single_value_extended_properties,
            ..graph("sunday")
        }
        .to_ical(PARIS);
        assert!(lines(&read).contains(&"RRULE:FREQ=WEEKLY;BYDAY=MO;INTERVAL=2;WKST=SU;COUNT=3"));
    }

    #[test]
    fn a_recurrence_without_a_date_is_stashed() {
        let doc = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:6\r\nRRULE:FREQ=DAILY\r\n\
            END:VTODO\r\nEND:VCALENDAR\r\n";

        let task = MsgraphTodoTask::from_ical(doc.as_bytes(), PARIS).unwrap();

        assert_eq!(task.recurrence, MsgraphField::Null);
        assert!(
            stash_lines(&task.single_value_extended_properties)
                .contains(&String::from("RRULE:FREQ=DAILY"))
        );
        assert!(lines(&stored(task).to_ical(PARIS)).contains(&"RRULE:FREQ=DAILY"));
    }

    #[test]
    fn an_absolute_alarm_is_the_reminder_and_others_survive_whole() {
        let doc = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:7\r\n\
            BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;RELATED=END:-PT15M\r\nDESCRIPTION:Soon\r\nEND:VALARM\r\n\
            BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=DATE-TIME:20261120T080000Z\r\nDESCRIPTION:Now\r\nEND:VALARM\r\n\
            END:VTODO\r\nEND:VCALENDAR\r\n";

        let task = MsgraphTodoTask::from_ical(doc.as_bytes(), PARIS).unwrap();
        assert_eq!(task.is_reminder_on, MsgraphField::Set(true));
        assert_eq!(
            task.reminder_date_time,
            MsgraphField::Set(MsgraphDateTimeTimeZone {
                date_time: "2026-11-20T08:00:00".into(),
                time_zone: Some("UTC".into()),
            })
        );

        let read = stored(task).to_ical(PARIS);
        assert!(read.contains(
            "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;RELATED=END:-PT15M\r\nDESCRIPTION:Soon\r\nEND:VALARM\r\n"
        ));
        assert!(read.contains("TRIGGER;VALUE=DATE-TIME:20261120T080000Z\r\n"));
    }

    #[test]
    fn a_document_without_a_task_is_refused() {
        let event = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:8\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let empty = "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n";
        let rule = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:9\r\nDUE;VALUE=DATE:20261109\r\n\
            RRULE:FREQ=HOURLY\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        assert!(matches!(
            MsgraphTodoTask::from_ical(event.as_bytes(), PARIS),
            Err(MsgraphTodoTaskIcalError::NotATodo(name)) if name == "VEVENT"
        ));
        assert!(matches!(
            MsgraphTodoTask::from_ical(empty.as_bytes(), PARIS),
            Err(MsgraphTodoTaskIcalError::NoTodo)
        ));
        assert!(matches!(
            MsgraphTodoTask::from_ical(rule.as_bytes(), PARIS),
            Err(MsgraphTodoTaskIcalError::UnsupportedRecurrence(rule)) if rule == "FREQ=HOURLY"
        ));
    }
}
