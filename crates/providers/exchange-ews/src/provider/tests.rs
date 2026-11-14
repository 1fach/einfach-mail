use std::{future::Future, future::ready};

use einfach_mail_application::{
    CalendarOperationError, CalendarProvider, ExchangeEwsAccountSettings,
    ExchangeEwsAccountSettingsStore, ExchangeEwsSettingsError, MailOperationError, MailProvider,
    MessageDeltaResponse,
};
use einfach_mail_domain::{
    AccountCapabilities, AccountId, CalendarCapabilities, CalendarEventCreation,
    CalendarEventDraft, CalendarEventUpdate, CalendarEventUpdateDraft, CalendarEventVersion,
    CalendarOccurrencePatch, CalendarOccurrenceTimePatch, CalendarTimeZoneId, CalendarViewRange,
    CalendarViewReference, EventId, FolderId, FolderReference, MailAction, MailCapabilities,
    MessageDeltaChange, MessageDeltaProgress, MessageId, MessageMetadata, MessageOrigin,
    MessagePreview, ReadState, UnreadCount, VersionedCalendarEvent,
};
use ews::server_version::ServerVersionInfo;
use ews::sync_folder_hierarchy::SyncFolderHierarchy;
use ews::sync_folder_items::{Change, SyncFolderItems};
use secrecy::SecretString;
use std::time::SystemTime;
use time::{Date, Month, PrimitiveDateTime, Time};

use crate::error::EwsError;
use crate::transport::{EwsTransport, parse_soap_response, serialize_soap_request};
use crate::wire::ExchangeProductVersion;

use super::{
    ExchangeEwsProvider,
    mail::{
        decode_cursor, encode_cursor, map_folder_change, map_item_change, map_sync_responses,
        message_sync_request,
    },
    session::{EwsSession, profile_for},
};
use crate::auth::decode_password;

const SYNC_FOLDER_ITEMS_PAGE: &[u8] = include_bytes!("..//fixtures/sync-folder-items-page.xml");
const SYNC_STATE_EXPIRED: &[u8] = include_bytes!("..//fixtures/sync-state-expired.xml");
const SYNC_FOLDER_ITEMS_PREVIEWS: &[u8] =
    include_bytes!("..//fixtures/sync-folder-items-previews.xml");
const SYNC_FOLDER_HIERARCHY_UNREAD_COUNTS: &[u8] =
    include_bytes!("..//fixtures/sync-folder-hierarchy-unread-counts.xml");

struct EmptySettingsStore;

impl ExchangeEwsAccountSettingsStore for EmptySettingsStore {
    fn load(
        &self,
    ) -> impl Future<Output = Result<Option<ExchangeEwsAccountSettings>, ExchangeEwsSettingsError>>
    + Send
    + '_ {
        ready(Ok(None))
    }

    fn save(
        &self,
        _: ExchangeEwsAccountSettings,
    ) -> impl Future<Output = Result<(), ExchangeEwsSettingsError>> + Send + '_ {
        ready(Ok(()))
    }

    fn delete(&self) -> impl Future<Output = Result<(), ExchangeEwsSettingsError>> + Send + '_ {
        ready(Ok(()))
    }
}

fn synthetic_calendar_view() -> CalendarViewReference {
    let date = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic calendar date is valid");
    let end_date = Date::from_calendar_date(2030, Month::January, 8)
        .expect("synthetic calendar end date is valid");
    let range = CalendarViewRange::new(
        date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        date,
        end_date,
    )
    .expect("synthetic calendar range is valid");
    CalendarViewReference::new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        range,
    )
}

fn synthetic_event_creation(view: &CalendarViewReference) -> CalendarEventCreation {
    let date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic event date is valid");
    let local_start = PrimitiveDateTime::new(
        date,
        Time::from_hms(9, 0, 0).expect("synthetic event start is valid"),
    );
    let local_end = PrimitiveDateTime::new(
        date,
        Time::from_hms(10, 0, 0).expect("synthetic event end is valid"),
    );
    let draft = CalendarEventDraft::timed(
        "Synthetic event".to_owned(),
        local_start,
        local_end,
        CalendarTimeZoneId::try_from("UTC".to_owned()).expect("synthetic time zone is valid"),
        local_start.assume_utc(),
        local_end.assume_utc(),
    )
    .expect("synthetic event draft is valid");
    CalendarEventCreation::try_new(view.clone(), draft)
        .expect("synthetic event fits the calendar view")
}

fn synthetic_versioned_event(view: &CalendarViewReference) -> VersionedCalendarEvent {
    let start = view.range().start() + time::Duration::hours(1);
    let end = start + time::Duration::hours(1);
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(start);
    time.set_end(end);
    time.set_is_all_day(false);
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Synthetic event".to_owned()));
    patch.set_time(time);
    let occurrence = patch
        .into_new(
            view.clone(),
            EventId::try_from("synthetic-event".to_owned()).expect("synthetic event ID is valid"),
        )
        .expect("synthetic timed event is valid");
    VersionedCalendarEvent::new(
        occurrence,
        CalendarEventVersion::try_from("synthetic-version".to_owned())
            .expect("synthetic event version is valid"),
    )
    .expect("synthetic event is editable")
}

include!("tests/account.rs");
include!("tests/mail.rs");
include!("tests/calendar.rs");
