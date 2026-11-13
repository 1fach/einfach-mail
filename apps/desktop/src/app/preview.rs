mod start;

use std::{
    future::{Future, ready},
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    features::{AccountServices, CalendarWeek},
    test_support::{calendar::TestCache, mail::FixtureCache},
};
use einfach_mail_application::{
    AccountCacheOpenError, AccountCacheRemovalError, AccountOperationCancellation,
    AccountOperationError, AccountProvider, AccountSession, AccountSessionFactory,
    AccountSessionFuture, AccountWorkflow, CalendarDeltaPageResponse, CalendarDeltaRequest,
    CalendarDeltaResponse, CalendarOperationError, CalendarProvider, CalendarSession,
    CalendarSyncStrategy, CalendarWorkflow, ConnectedAccount, MailOperationError, MailProvider,
    MailSendProviderError, MailSession, MailSyncState, MailWorkflow, MessageDeltaPageResponse,
    MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountCapabilities, AccountId, AccountProfile, CalendarCapabilities, CalendarDeltaLink,
    CalendarDeltaPage, CalendarDeltaProgress, CalendarEventCreation, CalendarEventUpdate,
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTimePatch,
    CalendarViewReference, EventId, FolderId, FolderReference, FollowUpFlagStatus,
    MailCapabilities, MailFolder, MailMessage, Mailbox, MessageBody, MessageBodyType,
    MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageId, MessageMetadata,
    MessageOrigin, ReadState, VersionedCalendarEvent,
};

pub(crate) use start::{
    APPEARANCE_ENV, DESTINATION_ENV, PreviewDestination, PreviewStart, WINDOW_SIZE_ENV,
};

pub(super) fn account_services(
    scenario: PreviewScenario,
) -> AccountServices<PreviewAccountProvider> {
    let account = preview_account();
    let account_id = account.profile().id().clone();
    let session = preview_session(account_id.clone(), scenario);
    let session_factory = Arc::new(PreviewAccountSessionFactory {
        account_id,
        session,
    });
    AccountServices::new(
        Arc::new(AccountWorkflow::new(PreviewAccountProvider { account })),
        session_factory,
    )
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum PreviewScenario {
    #[default]
    Standard,
    MailSyncRecovery,
}

impl PreviewScenario {
    pub(super) fn parse(value: Option<&str>) -> io::Result<Self> {
        match value {
            None => Ok(Self::Standard),
            Some("mail-sync-recovery") => Ok(Self::MailSyncRecovery),
            Some(value) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported UI preview scenario {value:?}"),
            )),
        }
    }
}

fn preview_session(account_id: AccountId, scenario: PreviewScenario) -> Arc<dyn AccountSession> {
    let mail_session = Arc::new(MailWorkflow::new(
        account_id.clone(),
        PreviewMailProvider::new(account_id.clone(), scenario),
        seed_mail_cache(account_id.clone()),
    )) as Arc<dyn MailSession>;
    let week = CalendarWeek::for_system(&account_id);
    let calendar_session = Arc::new(CalendarWorkflow::new(
        account_id.clone(),
        PreviewCalendarProvider::new(account_id.clone()),
        TestCache::with_events(preview_calendar_events(week.view())),
    )) as Arc<dyn CalendarSession>;
    Arc::new(PreviewAccountSession {
        mail: mail_session,
        calendar: calendar_session,
    })
}

fn preview_account() -> ConnectedAccount {
    let account_id = AccountId::try_from("preview-account".to_owned())
        .expect("the synthetic preview account ID is valid");
    let profile = AccountProfile::try_new(
        account_id,
        Some("Einfach Mail Preview".to_owned()),
        Some("preview@example.test".to_owned()),
        Some("preview@example.test".to_owned()),
        AccountCapabilities::new(
            MailCapabilities::read_only(),
            CalendarCapabilities::view_only(),
        ),
    )
    .expect("the synthetic preview account profile is valid");
    ConnectedAccount::new(
        profile,
        einfach_mail_application::AccountProviderKind::MicrosoftGraph,
    )
}

#[derive(Clone)]
pub(super) struct PreviewAccountProvider {
    account: ConnectedAccount,
}

impl AccountProvider for PreviewAccountProvider {
    async fn restore_account(&self) -> Result<Option<ConnectedAccount>, AccountOperationError> {
        Ok(Some(self.account.clone()))
    }

    async fn saved_credential_account_id(
        &self,
    ) -> Result<Option<AccountId>, AccountOperationError> {
        Ok(Some(self.account.profile().id().clone()))
    }

    async fn connect_account(
        &self,
        cancellation: AccountOperationCancellation,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        cancellation.begin_credential_commit()?;
        Ok(self.account.clone())
    }

    fn reauthorize_account(
        &self,
        expected_account_id: &AccountId,
        _request: einfach_mail_application::AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
        let expected_account_id = expected_account_id.clone();
        async move {
            if &expected_account_id != self.account.profile().id() {
                return Err(AccountOperationError::AccountMismatch);
            }
            cancellation.begin_credential_commit()?;
            Ok(self.account.clone())
        }
    }

    async fn sign_out(&self) -> Result<(), AccountOperationError> {
        Ok(())
    }
}

struct PreviewAccountSessionFactory {
    account_id: AccountId,
    session: Arc<dyn AccountSession>,
}

impl AccountSessionFactory for PreviewAccountSessionFactory {
    fn open_session(
        &self,
        account_id: AccountId,
    ) -> AccountSessionFuture<'_, Result<Arc<dyn AccountSession>, AccountCacheOpenError>> {
        Box::pin(async move {
            if account_id != self.account_id {
                return Err(AccountCacheOpenError::AccountMismatch);
            }
            Ok(Arc::clone(&self.session))
        })
    }

    fn remove_account_cache<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> AccountSessionFuture<'a, Result<(), AccountCacheRemovalError>> {
        let account_id = account_id.clone();
        let expected_account_id = self.account_id.clone();
        Box::pin(async move {
            if account_id != expected_account_id {
                return Err(AccountCacheRemovalError::AccountMismatch);
            }
            Ok(())
        })
    }
}

struct PreviewAccountSession {
    mail: Arc<dyn MailSession>,
    calendar: Arc<dyn CalendarSession>,
}

impl AccountSession for PreviewAccountSession {
    fn mail_session(&self) -> Arc<dyn MailSession> {
        Arc::clone(&self.mail)
    }

    fn calendar_session(&self) -> Arc<dyn CalendarSession> {
        Arc::clone(&self.calendar)
    }

    fn remove_account_cache(
        &self,
    ) -> AccountSessionFuture<'_, Result<(), AccountCacheRemovalError>> {
        Box::pin(async { Ok(()) })
    }
}

struct PreviewMailProvider {
    account_id: AccountId,
    folders: Vec<MailFolder>,
    fail_next_mail_sync: AtomicBool,
}

impl PreviewMailProvider {
    fn new(account_id: AccountId, scenario: PreviewScenario) -> Self {
        Self {
            folders: preview_mail_folders(&account_id),
            account_id,
            fail_next_mail_sync: AtomicBool::new(scenario == PreviewScenario::MailSyncRecovery),
        }
    }
}

impl MailProvider for PreviewMailProvider {
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let requested_account_id = account_id.clone();
        async move {
            if requested_account_id != self.account_id {
                return Err(MailOperationError::AccountMismatch);
            }
            Ok(self.folders.clone())
        }
    }

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        _request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        let folder_account_id = folder.account_id().clone();
        async move {
            if folder_account_id != self.account_id {
                return Err(MailOperationError::AccountMismatch);
            }
            if self.fail_next_mail_sync.swap(false, Ordering::Relaxed) {
                return Err(MailOperationError::ServiceUnavailable);
            }
            Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                MessageDeltaPage::new(
                    Vec::new(),
                    MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                        "preview-mail-delta-v1".to_owned(),
                    )),
                ),
                0,
            )))
        }
    }

    async fn fetch_message_body<'a>(
        &'a self,
        folder: &'a FolderReference,
        _message_id: &'a MessageId,
    ) -> Result<MessageBody, MailOperationError> {
        if folder.account_id() != &self.account_id {
            return Err(MailOperationError::AccountMismatch);
        }
        Ok(MessageBody::new(
            MessageBodyType::Text,
            "This synthetic message is included for the native UI preview.".to_owned(),
        ))
    }

    fn set_message_read_state(
        &self,
        _folder: &FolderReference,
        _message_id: &MessageId,
        _requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::UnsupportedOperation))
    }

    fn set_message_follow_up_flag_status(
        &self,
        _folder: &FolderReference,
        _message_id: &MessageId,
        _status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::UnsupportedOperation))
    }

    fn move_message(
        &self,
        _source_folder: &FolderReference,
        _message_id: &MessageId,
        _destination_folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::UnsupportedOperation))
    }

    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        let requested_account_id = account_id.clone();
        async move {
            if requested_account_id != self.account_id {
                return Err(MailOperationError::AccountMismatch);
            }
            let folder_id = match folder {
                WellKnownMailFolder::Archive => "archive",
                WellKnownMailFolder::DeletedItems => "deleted-items",
            };
            Ok(FolderReference::new(
                self.account_id.clone(),
                FolderId::try_from(folder_id.to_owned())
                    .expect("synthetic well-known folder ID is valid"),
            ))
        }
    }

    fn send_mail(
        &self,
        _account_id: &AccountId,
        _draft: einfach_mail_domain::MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        ready(Err(MailSendProviderError::UnsupportedOperation))
    }

    fn reply_to_message(
        &self,
        _account_id: &AccountId,
        _draft: einfach_mail_domain::MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        ready(Err(MailSendProviderError::UnsupportedOperation))
    }
}

struct PreviewCalendarProvider {
    account_id: AccountId,
}

impl PreviewCalendarProvider {
    fn new(account_id: AccountId) -> Self {
        Self { account_id }
    }
}

impl CalendarProvider for PreviewCalendarProvider {
    fn calendar_capabilities(
        &self,
    ) -> impl Future<Output = Result<CalendarCapabilities, CalendarOperationError>> + Send + '_
    {
        ready(Ok(CalendarCapabilities::view_only()))
    }

    fn calendar_sync_strategy(
        &self,
    ) -> impl Future<Output = Result<CalendarSyncStrategy, CalendarOperationError>> + Send + '_
    {
        ready(Ok(CalendarSyncStrategy::Delta))
    }

    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        _request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        let view_account_id = view.account_id().clone();
        async move {
            if view_account_id != self.account_id {
                return Err(CalendarOperationError::AccountMismatch);
            }
            Ok(CalendarDeltaResponse::Page(CalendarDeltaPageResponse::new(
                CalendarDeltaPage::new(
                    Vec::new(),
                    CalendarDeltaProgress::Complete(CalendarDeltaLink::from_opaque(
                        "preview-calendar-delta-v1".to_owned(),
                    )),
                ),
                0,
            )))
        }
    }

    fn create_event(
        &self,
        _creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn recover_event_creation(
        &self,
        _creation: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn load_versioned_event(
        &self,
        _event: &CalendarOccurrence,
    ) -> impl Future<Output = Result<VersionedCalendarEvent, CalendarOperationError>> + Send + '_
    {
        ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn update_event(
        &self,
        _update: &CalendarEventUpdate,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn delete_event(
        &self,
        _event: &VersionedCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        ready(Err(CalendarOperationError::UnsupportedOperation))
    }
}

fn preview_mail_folders(account_id: &AccountId) -> Vec<MailFolder> {
    [
        ("inbox", "Inbox"),
        ("archive", "Archive"),
        ("sent", "Sent Items"),
        ("drafts", "Drafts"),
    ]
    .into_iter()
    .map(|(folder_id, name)| {
        MailFolder::new(
            folder_reference(account_id, folder_id),
            None,
            name.to_owned(),
        )
        .expect("synthetic mail folder is valid")
    })
    .collect()
}

fn folder_reference(account_id: &AccountId, folder_id: &str) -> FolderReference {
    FolderReference::new(
        account_id.clone(),
        FolderId::try_from(folder_id.to_owned()).expect("synthetic folder ID is valid"),
    )
}

fn seed_mail_cache(account_id: AccountId) -> FixtureCache {
    let cache = FixtureCache::default();
    let folders = preview_mail_folders(&account_id);
    let inbox = folders
        .first()
        .expect("the preview account has an Inbox folder")
        .reference()
        .clone();
    let now = SystemTime::now();
    let messages = preview_messages(&inbox, now);
    let sync_state = MailSyncState::from_checkpoint(
        UNIX_EPOCH,
        Some(MessageDeltaLink::from_opaque(
            "preview-mail-delta-v1".to_owned(),
        )),
        None,
        Some(now),
        None,
        0,
    )
    .expect("synthetic mail synchronization checkpoint is valid");
    {
        let mut fixture = cache.0.lock().expect("preview mail cache mutex is healthy");
        fixture.account_id = Some(account_id);
        fixture.folders = folders;
        fixture.messages = messages;
        fixture.body = Some(MessageBody::new(
            MessageBodyType::Text,
            "This synthetic message is included for the native UI preview.".to_owned(),
        ));
        fixture.sync_state = Some(sync_state);
    }
    cache
}

#[derive(Clone, Copy)]
struct PreviewMessageSeed {
    id: &'static str,
    sender_name: &'static str,
    sender_address: &'static str,
    subject: &'static str,
    hours_ago: u64,
    unread: bool,
    flagged: bool,
}

/// Newest first, spread so the list shows today, yesterday, a weekday, a date in the current
/// year, and a date in an earlier year.
const PREVIEW_MESSAGE_SEEDS: &[PreviewMessageSeed] = &[
    PreviewMessageSeed {
        id: "preview-message-1",
        sender_name: "Mira Example",
        sender_address: "mira@example.test",
        subject: "Welcome to the Einfach Mail preview",
        hours_ago: 1,
        unread: true,
        flagged: false,
    },
    PreviewMessageSeed {
        id: "preview-message-2",
        sender_name: "Jonas Example",
        sender_address: "jonas@example.test",
        subject: "Calendar planning for this week",
        hours_ago: 2,
        unread: false,
        flagged: true,
    },
    PreviewMessageSeed {
        id: "preview-message-3",
        sender_name: "Ada Example",
        sender_address: "ada@example.test",
        subject: "Notes from the product review",
        hours_ago: 26,
        unread: false,
        flagged: false,
    },
    PreviewMessageSeed {
        id: "preview-message-4",
        sender_name: "Lea Example",
        sender_address: "lea@example.test",
        subject: "Synthetic invoice example",
        hours_ago: 30,
        unread: true,
        flagged: true,
    },
    PreviewMessageSeed {
        id: "preview-message-5",
        sender_name: "Noah Example",
        sender_address: "noah@example.test",
        subject: "The message body is synthetic",
        hours_ago: 72,
        unread: false,
        flagged: false,
    },
    PreviewMessageSeed {
        id: "preview-message-6",
        sender_name: "Priya Example",
        sender_address: "priya@example.test",
        subject: "Planning notes for the next quarter",
        hours_ago: 288,
        unread: false,
        flagged: false,
    },
    PreviewMessageSeed {
        id: "preview-message-7",
        sender_name: "Omar Example",
        sender_address: "omar@example.test",
        subject: "Summary of last year's synthetic archive",
        hours_ago: 9_000,
        unread: false,
        flagged: false,
    },
];

fn preview_messages(folder: &FolderReference, now: SystemTime) -> Vec<MailMessage> {
    PREVIEW_MESSAGE_SEEDS
        .iter()
        .map(|seed| preview_message(folder, now, *seed))
        .collect()
}

fn preview_message(
    folder: &FolderReference,
    now: SystemTime,
    seed: PreviewMessageSeed,
) -> MailMessage {
    let sender = Mailbox::try_new(
        seed.sender_address.to_owned(),
        Some(seed.sender_name.to_owned()),
    )
    .expect("synthetic sender mailbox is valid");
    let message_id =
        MessageId::try_from(seed.id.to_owned()).expect("synthetic message ID is valid");
    let read_state = if seed.unread {
        ReadState::Unread
    } else {
        ReadState::Read
    };
    let metadata = MessageMetadata::new(
        Some(seed.subject.to_owned()),
        MessageOrigin::new(Some(sender), None),
        now - std::time::Duration::from_secs(seed.hours_ago * 60 * 60),
        read_state,
        seed.flagged.then_some(FollowUpFlagStatus::Flagged),
    );
    MailMessage::new(folder.clone(), message_id, metadata)
}

/// A timed preview event: its day of the week (Monday is 0), start hour and minute, and length.
struct PreviewTimedEvent {
    id: &'static str,
    subject: &'static str,
    location: &'static str,
    day: i64,
    hour: i64,
    minute: i64,
    minutes: i64,
}

const PREVIEW_TIMED_EVENTS: [PreviewTimedEvent; 10] = [
    PreviewTimedEvent {
        id: "preview-event-1",
        subject: "Team stand-up",
        location: "Room 1",
        day: 0,
        hour: 9,
        minute: 0,
        minutes: 30,
    },
    PreviewTimedEvent {
        id: "preview-event-2",
        subject: "Design review",
        location: "Room 4",
        day: 0,
        hour: 11,
        minute: 0,
        minutes: 60,
    },
    PreviewTimedEvent {
        id: "preview-event-3",
        subject: "Project planning",
        location: "Room 2",
        day: 1,
        hour: 14,
        minute: 0,
        minutes: 90,
    },
    PreviewTimedEvent {
        id: "preview-event-4",
        subject: "Team stand-up",
        location: "Room 1",
        day: 1,
        hour: 9,
        minute: 0,
        minutes: 30,
    },
    PreviewTimedEvent {
        id: "preview-event-5",
        subject: "Team stand-up",
        location: "Room 1",
        day: 2,
        hour: 9,
        minute: 0,
        minutes: 30,
    },
    PreviewTimedEvent {
        id: "preview-event-6",
        subject: "Customer workshop",
        location: "Studio",
        day: 2,
        hour: 10,
        minute: 30,
        minutes: 120,
    },
    PreviewTimedEvent {
        id: "preview-event-7",
        subject: "Budget check-in",
        location: "Room 3",
        day: 2,
        hour: 15,
        minute: 30,
        minutes: 45,
    },
    PreviewTimedEvent {
        id: "preview-event-8",
        subject: "Team stand-up",
        location: "Room 1",
        day: 3,
        hour: 9,
        minute: 0,
        minutes: 30,
    },
    PreviewTimedEvent {
        id: "preview-event-9",
        subject: "Release readiness",
        location: "Room 2",
        day: 4,
        hour: 13,
        minute: 0,
        minutes: 60,
    },
    PreviewTimedEvent {
        id: "preview-event-10",
        subject: "Weekly retro",
        location: "Room 1",
        day: 4,
        hour: 16,
        minute: 0,
        minutes: 45,
    },
];

fn preview_calendar_events(view: &CalendarViewReference) -> Vec<CalendarOccurrence> {
    let mut events = PREVIEW_TIMED_EVENTS
        .iter()
        .map(|event| preview_timed_event(view, event))
        .collect::<Vec<_>>();
    events.push(preview_all_day_event(view));
    events
}

fn preview_timed_event(
    view: &CalendarViewReference,
    event: &PreviewTimedEvent,
) -> CalendarOccurrence {
    let start = view.range().start()
        + time::Duration::days(event.day)
        + time::Duration::hours(event.hour)
        + time::Duration::minutes(event.minute);
    let mut event_time = CalendarOccurrenceTimePatch::new();
    event_time.set_start(start);
    event_time.set_end(start + time::Duration::minutes(event.minutes));
    event_time.set_is_all_day(false);
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(event.subject.to_owned()));
    patch.set_location(Some(event.location.to_owned()));
    patch.set_time(event_time);
    patch
        .into_new(
            view.clone(),
            EventId::try_from(event.id.to_owned()).expect("synthetic event ID is valid"),
        )
        .expect("synthetic calendar event is valid")
}

/// An all-day event on Thursday and Friday, so the week has one that spans two columns.
fn preview_all_day_event(view: &CalendarViewReference) -> CalendarOccurrence {
    let first_day = view.range().start_date().midnight().assume_utc() + time::Duration::days(3);
    let mut event_time = CalendarOccurrenceTimePatch::new();
    event_time.set_start(first_day);
    event_time.set_end(first_day + time::Duration::days(2));
    event_time.set_is_all_day(true);
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Team offsite".to_owned()));
    patch.set_time(event_time);
    patch
        .into_new(
            view.clone(),
            EventId::try_from("preview-event-all-day".to_owned())
                .expect("synthetic event ID is valid"),
        )
        .expect("synthetic all-day event is valid")
}

#[cfg(test)]
mod tests;
