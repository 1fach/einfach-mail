//! Live tests against Microsoft Graph: mail, contacts, calendars and To Do.
//!
//! Graph only accepts an OAuth 2.0 Bearer token. A token minted by hand
//! (delegated, `me` by default) dies within the hour:
//!
//! ```sh
//! MSGRAPH_ACCESS_TOKEN="eyJ0..." \
//! cargo test --features ical,vcard --test msgraph -- --ignored
//! ```
//!
//! An app registration with a client secret instead mints its own token
//! through the client credentials grant, which is what makes an
//! unattended run possible. The tests then act app-only on one mailbox,
//! `MSGRAPH_USER_ID` (`microsoft@pimalaya.onmicrosoft.com` by default,
//! the Pimalaya test mailbox):
//!
//! ```sh
//! MSGRAPH_TENANT_ID=… MSGRAPH_CLIENT_ID=… MSGRAPH_CLIENT_SECRET=… \
//! cargo test --features ical,vcard --test msgraph -- --ignored
//! ```
//!
//! The app needs the Graph application permissions `Mail.ReadWrite`,
//! `Mail.Send`, `Contacts.ReadWrite`, `Calendars.ReadWrite`,
//! `Tasks.ReadWrite.All` and `User.Read.All`, admin consent granted.
//!
//! [`account`] only reads. [`mail`], [`contacts`], [`calendars`] and
//! [`todo`] walk the CRUD surface inside a throwaway folder, calendar or
//! task list that [`with_cleanup`] deletes however the run ends; the
//! messages sent to the mailbox itself land in its inbox and sent items,
//! and the cleanup sweeps them by subject. With the `ical` and `vcard`
//! features, [`ical`], [`ical_todo`] and [`vcard`] round-trip an event, a
//! task and a contact through the projections, and [`ical_instances`]
//! writes single occurrences of a series through their instances. Every
//! resource a run creates is named `io-msgraph-test-<millis>`, and nothing
//! else is ever written.

#![cfg(any(
    feature = "rustls-ring",
    feature = "rustls-aws",
    feature = "native-tls"
))]

use core::fmt::Debug;
use std::{
    borrow::Cow,
    env,
    panic::{self, AssertUnwindSafe},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use io_msgraph::v1::{
    client::{MsgraphClientStd, MsgraphClientStdConnectOptions, MsgraphClientStdError},
    field::MsgraphField,
    rest::{
        batch::MsgraphBatchRequest,
        users::{
            calendars::MsgraphCalendar,
            contact_folders::MsgraphContactFolder,
            contacts::{MsgraphContact, list::MsgraphContactsListParams},
            events::{
                MsgraphDateTimeTimeZone, MsgraphDayOfWeek, MsgraphEvent, MsgraphEventType,
                MsgraphPatternedRecurrence, MsgraphRecurrencePattern, MsgraphRecurrencePatternType,
                MsgraphRecurrenceRange, MsgraphRecurrenceRangeType, list::MsgraphEventsListParams,
            },
            mail_folders::MsgraphMailFolder,
            messages::{
                MsgraphBodyType, MsgraphEmailAddress, MsgraphImportance, MsgraphItemBody,
                MsgraphMessage, MsgraphRecipient, list::MsgraphMessagesListParams,
            },
            todo::{
                lists::{MsgraphTodoList, MsgraphTodoWellknownListName},
                tasks::{MsgraphLinkedResource, MsgraphTodoTask, MsgraphTodoTaskStatus},
            },
        },
    },
};
use io_oauth::{client::Oauth20ClientStd, rfc6749::client_credentials::*};
use pimalaya_stream::tls::Tls;
use secrecy::{ExposeSecret, SecretString};
use url::Url;

/// The scope of an app-only Graph token: every application permission
/// the app was granted.
const GRAPH_SCOPE: &str = "https://graph.microsoft.com/.default";

/// The Pimalaya test mailbox an app-only run acts on.
const DEFAULT_USER: &str = "microsoft@pimalaya.onmicrosoft.com";

/// The window every calendar test works in, far enough ahead that no
/// real appointment of the mailbox falls in it.
const WINDOW_START: &str = "2030-01-01T00:00:00Z";
const WINDOW_END: &str = "2030-02-01T00:00:00Z";

/// How long a step waits for Graph to reflect a write it already
/// acknowledged.
const SETTLE: Duration = Duration::from_secs(120);

#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn account() {
    let mut client = connect();

    let me = client.me().expect("me").response;
    let address = me
        .mail
        .clone()
        .or_else(|| me.user_principal_name.clone())
        .expect("the mailbox exposes an address");
    assert!(address.contains('@'));

    let folders = client
        .mail_folders_list(&Default::default())
        .expect("mail folders list")
        .response;
    assert!(!folders.value.is_empty(), "a mailbox has mail folders");

    let calendars = client
        .calendars_list(&Default::default())
        .expect("calendars list")
        .response;
    assert!(
        calendars
            .value
            .iter()
            .any(|calendar| calendar.is_default_calendar == Some(true)),
        "a mailbox has a default calendar"
    );

    client
        .contact_folders_list(&Default::default())
        .expect("contact folders list");
}

#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn mail() {
    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());
    let address = address(&mut client);

    let folder = client
        .mail_folder_create(&MsgraphMailFolder {
            display_name: name.clone(),
            ..Default::default()
        })
        .expect("mail folder create")
        .response;
    assert_eq!(folder.display_name, name);
    let folder_id = folder.id;
    let sent_subject = format!("{name} sent");

    with_cleanup(
        &mut client,
        |client| {
            mail_folders(client, &folder_id, &name);
            messages(client, &folder_id, &name, &address);
            messages_delta(client, &folder_id, &name);
            errors_and_batch(client, &folder_id, &name, &address);
            send(client, &address, &sent_subject);
        },
        |client| {
            sweep(client, &sent_subject);
            if let Err(err) = client.mail_folder_delete(&folder_id) {
                report_leftover("mail folder", &folder_id, &err);
            }
        },
    );
}

#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn contacts() {
    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());

    let folder_id = contact_folder_create(&mut client, &name);

    with_cleanup(
        &mut client,
        |client| {
            contact_folders(client, &folder_id, &name);
            contact_crud(client, &folder_id, &name);
            contacts_delta(client, &folder_id, &name);
        },
        |client| {
            if let Err(err) = client.contact_folder_delete(&folder_id) {
                report_leftover("contact folder", &folder_id, &err);
            }
        },
    );
}

#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn calendars() {
    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());

    let calendar_id = calendar_create(&mut client, &name);

    with_cleanup(
        &mut client,
        |client| {
            calendar_metadata(client, &calendar_id, &name);
            single_event(client, &calendar_id, &name);
            events_paging(client, &calendar_id, &name);
            recurring_event(client, &calendar_id, &name);
            events_delta(client, &calendar_id, &name);
        },
        |client| {
            if let Err(err) = client.calendar_delete(&calendar_id) {
                report_leftover("calendar", &calendar_id, &err);
            }
        },
    );
}

#[cfg(feature = "ical")]
#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn ical() {
    use io_msgraph::v1::rest::users::events::ical::{
        MSGRAPH_EVENT_ICAL_SELECT, MSGRAPH_EVENT_STASH_EXPAND,
    };

    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());
    let uid = format!("{name}@pimalaya.org");

    let calendar_id = calendar_create(&mut client, &name);

    with_cleanup(
        &mut client,
        |client| {
            let document = ical_document(&uid, &name, true);
            let written =
                MsgraphEvent::create_from_ical(document.as_bytes()).expect("the document projects");
            let created = client
                .event_create(Some(&calendar_id), &written)
                .expect("event create")
                .response;

            let fetched = client
                .event_get(
                    &created.id,
                    Some(MSGRAPH_EVENT_ICAL_SELECT),
                    Some(MSGRAPH_EVENT_STASH_EXPAND),
                )
                .expect("event get")
                .response;
            assert_eq!(
                fetched.stashed_uid().as_deref(),
                Some(uid.as_str()),
                "the UID rides the stash"
            );
            let read = fetched.to_ical();
            assert!(
                read.contains("X-PIMALAYA-TEST:kept verbatim"),
                "the stash restores the unmanaged lines:\n{read}"
            );
            assert_same_event(&document, &read);

            let edited = ical_document(&uid, &format!("{name} renamed"), false);
            let patch = MsgraphEvent::update_from_ical(edited.as_bytes(), read.as_bytes())
                .expect("the edited document projects");
            client
                .event_update(&created.id, &patch)
                .expect("event update");

            let refetched = client
                .event_get(
                    &created.id,
                    Some(MSGRAPH_EVENT_ICAL_SELECT),
                    Some(MSGRAPH_EVENT_STASH_EXPAND),
                )
                .expect("event get after update")
                .response;
            let reread = refetched.to_ical();
            assert_same_event(&edited, &reread);

            // NOTE: what an editor does to a document read from Graph:
            // the text changes, the X-ALT-DESC beside it does not.
            let annotated = reread.replace(
                "DESCRIPTION:written by the io-msgraph suite",
                "DESCRIPTION:annotated by hand",
            );
            assert_ne!(annotated, reread, "the read document carries the text");
            let patch = MsgraphEvent::update_from_ical(annotated.as_bytes(), reread.as_bytes())
                .expect("the annotated document projects");
            client
                .event_update(&created.id, &patch)
                .expect("event update of the description");

            let annotated_read = client
                .event_get(
                    &created.id,
                    Some(MSGRAPH_EVENT_ICAL_SELECT),
                    Some(MSGRAPH_EVENT_STASH_EXPAND),
                )
                .expect("event get after the description update")
                .response
                .to_ical();
            assert_eq!(
                description(&annotated_read),
                Some("annotated by hand"),
                "the edited text reaches Graph:\n{annotated_read}"
            );
        },
        |client| {
            if let Err(err) = client.calendar_delete(&calendar_id) {
                report_leftover("calendar", &calendar_id, &err);
            }
        },
    );
}

/// A series read the way calendula and neverest read one: its master and
/// its exceptions, from the default instance listing, through
/// `to_ical_series`.
#[cfg(feature = "ical")]
#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn ical_series() {
    use io_msgraph::v1::rest::users::events::ical::{
        MSGRAPH_EVENT_ICAL_SELECT, MSGRAPH_EVENT_STASH_EXPAND,
    };

    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());
    let uid = format!("{name}@pimalaya.org");
    let address = address(&mut client);

    let calendar_id = calendar_create(&mut client, &name);

    with_cleanup(
        &mut client,
        |client| {
            let document = event_document(
                &uid,
                &name,
                &[
                    "DTSTART;TZID=Europe/Paris:20300110T100000",
                    "DTEND;TZID=Europe/Paris:20300110T110000",
                    "RRULE:FREQ=WEEKLY;COUNT=3",
                    &format!("ATTENDEE;CN=Pimalaya;ROLE=REQ-PARTICIPANT:mailto:{address}"),
                ],
            );
            let written =
                MsgraphEvent::create_from_ical(document.as_bytes()).expect("the series projects");
            let master = client
                .event_create(Some(&calendar_id), &written)
                .expect("series create")
                .response;

            let mut instances = client
                .event_instances(&master.id, WINDOW_START, WINDOW_END, &Default::default())
                .expect("series instances")
                .response
                .value;
            assert_eq!(instances.len(), 3, "three weekly instances");
            instances.sort_by(|a, b| {
                a.start
                    .as_option()
                    .map(|s| &s.date_time)
                    .cmp(&b.start.as_option().map(|s| &s.date_time))
            });

            let moved = format!("{name} moved");
            client
                .event_update(
                    &instances[1].id,
                    &MsgraphEvent {
                        subject: MsgraphField::Set(moved.clone()),
                        ..Default::default()
                    },
                )
                .expect("instance update");
            client
                .event_delete(&instances[2].id)
                .expect("instance delete");

            let master = client
                .event_get(
                    &master.id,
                    Some(MSGRAPH_EVENT_ICAL_SELECT),
                    Some(MSGRAPH_EVENT_STASH_EXPAND),
                )
                .expect("series master get")
                .response;
            // NOTE: an exception needs its originalStart for its
            // RECURRENCE-ID, which the default listing leaves out.
            let params = MsgraphEventsListParams {
                select: Some(MSGRAPH_EVENT_ICAL_SELECT),
                ..Default::default()
            };
            let instances = client
                .event_instances(&master.id, WINDOW_START, WINDOW_END, &params)
                .expect("series instances after edits")
                .response
                .value;
            let exceptions: Vec<&MsgraphEvent> = instances
                .iter()
                .filter(|e| e.event_type == Some(MsgraphEventType::Exception))
                .collect();
            assert_eq!(exceptions.len(), 1, "one exception: {instances:?}");

            let read = master.to_ical_series(&exceptions);
            for expected in [
                "RECURRENCE-ID;TZID=Europe/Paris:20300117T100000\r\n",
                "EXDATE;TZID=Europe/Paris:20300124T100000\r\n",
                &format!("SUMMARY:{moved}\r\n"),
            ] {
                assert!(read.contains(expected), "missing `{expected}`:\n{read}");
            }
            assert!(
                read.to_lowercase()
                    .contains(&format!("mailto:{}", address.to_lowercase())),
                "the attendee reads back:\n{read}"
            );
            MsgraphEvent::from_ical(read.as_bytes()).expect("the series document projects back");
        },
        |client| {
            if let Err(err) = client.calendar_delete(&calendar_id) {
                report_leftover("calendar", &calendar_id, &err);
            }
        },
    );
}

/// One occurrence written through its instance, as a calendar client
/// pushes an override: moved, deleted, reverted, and edited beside its
/// series, each read back through `to_ical_series`.
#[cfg(feature = "ical")]
#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn ical_instances() {
    use io_msgraph::v1::rest::users::events::ical::{
        MSGRAPH_EVENT_ICAL_SELECT, MSGRAPH_EVENT_STASH_EXPAND,
    };

    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());
    let uid = format!("{name}@pimalaya.org");
    let calendar_id = calendar_create(&mut client, &name);

    // NOTE: the occurrence of 10:00 in Paris on `day`, alone, starting at
    // `hour` and carrying `extra`.
    let occurrence = |day: &str, hour: &str, extra: &[&str]| {
        let mut timing = vec![
            format!("RECURRENCE-ID;TZID=Europe/Paris:{day}T100000"),
            format!("DTSTART;TZID=Europe/Paris:{day}T{hour}0000"),
            format!("DTEND;TZID=Europe/Paris:{day}T{hour}3000"),
        ];
        timing.extend(extra.iter().map(|line| String::from(*line)));
        let timing: Vec<&str> = timing.iter().map(String::as_str).collect();
        event_document(&uid, &name, &timing)
    };

    with_cleanup(
        &mut client,
        |client| {
            let document = event_document(
                &uid,
                &name,
                &[
                    "DTSTART;TZID=Europe/Paris:20300110T100000",
                    "DTEND;TZID=Europe/Paris:20300110T103000",
                    "RRULE:FREQ=WEEKLY;COUNT=4",
                ],
            );
            let written =
                MsgraphEvent::create_from_ical(document.as_bytes()).expect("the series projects");
            let master_id = client
                .event_create(Some(&calendar_id), &written)
                .expect("series create")
                .response
                .id;

            let params = MsgraphEventsListParams {
                select: Some(MSGRAPH_EVENT_ICAL_SELECT),
                ..Default::default()
            };
            let read = |client: &mut MsgraphClientStd| {
                let master = client
                    .event_get(
                        &master_id,
                        Some(MSGRAPH_EVENT_ICAL_SELECT),
                        Some(MSGRAPH_EVENT_STASH_EXPAND),
                    )
                    .expect("series master get")
                    .response;
                let instances = client
                    .event_instances(&master_id, WINDOW_START, WINDOW_END, &params)
                    .expect("series instances")
                    .response
                    .value;
                (master, instances)
            };
            let instance_of = |master: &MsgraphEvent, instances: &[MsgraphEvent], day: &str| {
                instances
                    .iter()
                    .find(|instance| {
                        instance
                            .original_start
                            .as_deref()
                            .and_then(|start| master.recurrence_id_of(start))
                            .as_deref()
                            == Some(&format!("{day}T100000"))
                    })
                    .cloned()
                    .unwrap_or_else(|| panic!("no instance on {day}: {instances:?}"))
            };
            let series = |master: &MsgraphEvent, instances: &[MsgraphEvent]| {
                let exceptions: Vec<&MsgraphEvent> = instances
                    .iter()
                    .filter(|e| e.event_type == Some(MsgraphEventType::Exception))
                    .collect();
                master.to_ical_series(&exceptions)
            };

            // Moved: only the times reach the instance.
            let (master, instances) = read(client);
            let plain = occurrence("20300117", "10", &[]);
            let moved = occurrence("20300117", "12", &[]);
            let patch = MsgraphEvent::instance_update_from_ical(moved.as_bytes(), plain.as_bytes())
                .expect("the occurrence projects")
                .expect("a move is a change");
            assert!(patch.subject.is_unset() && patch.recurrence.is_unset());
            let target = instance_of(&master, &instances, "20300117");
            client
                .event_update(&target.id, &patch)
                .expect("instance update");

            // Deleted: the instance goes, the series reads an EXDATE.
            let target = instance_of(&master, &instances, "20300124");
            client.event_delete(&target.id).expect("instance delete");

            let (master, instances) = read(client);
            let document = series(&master, &instances);
            for expected in [
                "RECURRENCE-ID;TZID=Europe/Paris:20300117T100000\r\n",
                "DTSTART;TZID=Europe/Paris:20300117T120000\r\n",
                "EXDATE;TZID=Europe/Paris:20300124T100000\r\n",
            ] {
                assert!(
                    document.contains(expected),
                    "missing `{expected}`:\n{document}"
                );
            }

            // Reverted: the exception as Graph holds it, patched back to
            // the series' own times.
            let exception = instance_of(&master, &instances, "20300117");
            let held = exception.to_ical();
            let patch = MsgraphEvent::instance_update_from_ical(plain.as_bytes(), held.as_bytes())
                .expect("the occurrence projects")
                .expect("a revert is a change");
            client
                .event_update(&exception.id, &patch)
                .expect("instance revert");

            // The series and one occurrence, in one push: the master's
            // patch, then the instance's.
            let renamed = format!("{name} renamed");
            let edited = document.replace(
                &format!("SUMMARY:{name}\r\n"),
                &format!("SUMMARY:{renamed}\r\n"),
            );
            let patch = MsgraphEvent::update_from_ical(edited.as_bytes(), document.as_bytes())
                .expect("the series projects");
            assert_eq!(patch.subject.as_option(), Some(&renamed));
            client
                .event_update(&master_id, &patch)
                .expect("series update");
            let (master, instances) = read(client);
            let plain = occurrence("20300131", "10", &[]);
            let located = occurrence("20300131", "10", &["LOCATION:Room 3"]);
            let patch =
                MsgraphEvent::instance_update_from_ical(located.as_bytes(), plain.as_bytes())
                    .expect("the occurrence projects")
                    .expect("a location is a change");
            let target = instance_of(&master, &instances, "20300131");
            client
                .event_update(&target.id, &patch)
                .expect("instance update beside the series");

            let (master, instances) = read(client);
            let document = series(&master, &instances);
            assert!(
                !document.contains("DTSTART;TZID=Europe/Paris:20300117T120000\r\n"),
                "the revert reaches Graph:\n{document}"
            );
            assert!(
                document.contains(&format!("SUMMARY:{renamed}\r\n")),
                "the series edit reaches Graph:\n{document}"
            );
            assert!(
                document.contains("LOCATION:Room 3\r\n"),
                "the occurrence edit reaches Graph:\n{document}"
            );
            assert!(
                document.contains("EXDATE;TZID=Europe/Paris:20300124T100000\r\n"),
                "the deletion stands:\n{document}"
            );
        },
        |client| {
            if let Err(err) = client.calendar_delete(&calendar_id) {
                report_leftover("calendar", &calendar_id, &err);
            }
        },
    );
}

/// The shapes the weekly series of [`ical`] does not reach: an all-day
/// event, a monthly rule ending on a date, and a relative monthly rule.
#[cfg(feature = "ical")]
#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn ical_shapes() {
    use io_msgraph::v1::rest::users::events::ical::{
        MSGRAPH_EVENT_ICAL_SELECT, MSGRAPH_EVENT_STASH_EXPAND,
    };

    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());

    let calendar_id = calendar_create(&mut client, &name);

    with_cleanup(
        &mut client,
        |client| {
            let shapes: [(&str, &[&str]); 3] = [
                (
                    "all-day",
                    &["DTSTART;VALUE=DATE:20300112", "DTEND;VALUE=DATE:20300113"],
                ),
                (
                    "monthly-until",
                    &[
                        "DTSTART;TZID=Europe/Paris:20300115T100000",
                        "DTEND;TZID=Europe/Paris:20300115T110000",
                        "RRULE:FREQ=MONTHLY;BYMONTHDAY=15;UNTIL=20300615T080000Z",
                    ],
                ),
                (
                    "relative-monthly",
                    &[
                        "DTSTART;TZID=Europe/Paris:20300108T100000",
                        "DTEND;TZID=Europe/Paris:20300108T110000",
                        "RRULE:FREQ=MONTHLY;BYDAY=2TU;COUNT=3",
                    ],
                ),
            ];

            for (shape, timing) in shapes {
                let uid = format!("{name}-{shape}@pimalaya.org");
                let document = event_document(&uid, &format!("{name} {shape}"), timing);
                let written = MsgraphEvent::create_from_ical(document.as_bytes())
                    .unwrap_or_else(|err| panic!("the {shape} document projects: {err}"));
                let created = client
                    .event_create(Some(&calendar_id), &written)
                    .unwrap_or_else(|err| panic!("{shape} create: {err}"))
                    .response;

                let fetched = client
                    .event_get(
                        &created.id,
                        Some(MSGRAPH_EVENT_ICAL_SELECT),
                        Some(MSGRAPH_EVENT_STASH_EXPAND),
                    )
                    .unwrap_or_else(|err| panic!("{shape} get: {err}"))
                    .response;
                assert_same_event(&document, &fetched.to_ical());
            }
        },
        |client| {
            if let Err(err) = client.calendar_delete(&calendar_id) {
                report_leftover("calendar", &calendar_id, &err);
            }
        },
    );
}

#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn todo() {
    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());

    let list_id = todo_list_create(&mut client, &name);

    with_cleanup(
        &mut client,
        |client| {
            todo_list_metadata(client, &list_id, &name);
            todo_task_crud(client, &list_id, &name);
            todo_tasks_delta(client, &list_id, &name);
        },
        |client| {
            if let Err(err) = client.todo_list_delete(&list_id) {
                report_leftover("to do list", &list_id, &err);
            }
        },
    );
}

/// A task document written as the Android app writes one, round-tripped
/// through a To Do list, then edited.
#[cfg(feature = "ical")]
#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn ical_todo() {
    use io_msgraph::v1::rest::users::todo::tasks::{
        MsgraphTodoTaskStatus, ical::MSGRAPH_TODO_TASK_STASH_EXPAND,
    };

    const ZONE: &str = "Europe/Paris";

    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());
    let uid = format!("{name}@pimalaya.org");

    let list_id = todo_list_create(&mut client, &name);

    with_cleanup(
        &mut client,
        |client| {
            let document = todo_document(
                &uid,
                &[
                    &format!("SUMMARY:{name}"),
                    "DESCRIPTION:written by the io-msgraph suite",
                    "DUE;VALUE=DATE:20300115",
                    "PRIORITY:5",
                    "STATUS:NEEDS-ACTION",
                    "CATEGORIES:inbox,@phone",
                    "CONTACT;ALTREP=\"mailto:lea@example.net\":Léa Martin",
                    "RELATED-TO;RELTYPE=PARENT:project-1",
                    "ATTACH:mid:abc%2F1@example.net",
                    "X-PIMALAYA-TEST:kept verbatim",
                ],
            );
            let written = MsgraphTodoTask::create_from_ical(document.as_bytes(), ZONE)
                .expect("the document projects");
            let created = client
                .todo_task_create(&list_id, &written)
                .expect("todo task create")
                .response;

            let fetched = client
                .todo_task_get(
                    &list_id,
                    &created.id,
                    None,
                    Some(MSGRAPH_TODO_TASK_STASH_EXPAND),
                )
                .expect("todo task get")
                .response;
            assert_eq!(
                fetched.stashed_uid().as_deref(),
                Some(uid.as_str()),
                "the UID rides the stash"
            );
            let read = fetched.to_ical(ZONE);
            assert_eq!(todo_lines(&read), todo_lines(&document), "{read}");

            let edited = read
                .replace(
                    &format!("SUMMARY:{name}"),
                    &format!("SUMMARY:{name} renamed"),
                )
                .replace(
                    "STATUS:NEEDS-ACTION",
                    "STATUS:NEEDS-ACTION\r\nX-MSGRAPH-STATUS:waitingOnOthers",
                )
                .replace(
                    "DUE;VALUE=DATE:20300115",
                    "DUE;TZID=Europe/Paris:20300116T143000",
                );
            let patch = MsgraphTodoTask::update_from_ical(edited.as_bytes(), read.as_bytes(), ZONE)
                .expect("the edited document projects");
            client
                .todo_task_update(&list_id, &created.id, &patch)
                .expect("todo task update");

            let refetched = client
                .todo_task_get(
                    &list_id,
                    &created.id,
                    None,
                    Some(MSGRAPH_TODO_TASK_STASH_EXPAND),
                )
                .expect("todo task get after update")
                .response;
            assert_eq!(
                refetched.status,
                MsgraphField::Set(MsgraphTodoTaskStatus::WaitingOnOthers)
            );
            let reread = refetched.to_ical(ZONE);
            assert_eq!(todo_lines(&reread), todo_lines(&edited), "{reread}");
        },
        |client| {
            if let Err(err) = client.todo_list_delete(&list_id) {
                report_leftover("to do list", &list_id, &err);
            }
        },
    );
}

#[cfg(feature = "vcard")]
#[test]
#[ignore = "requires Graph credentials and --ignored"]
fn vcard() {
    use io_msgraph::v1::rest::users::contacts::vcard::MSGRAPH_CONTACT_STASH_EXPAND;

    let mut client = connect();
    let name = format!("io-msgraph-test-{}", unix_millis());
    let uid = format!("urn:uuid:{name}");

    let folder_id = contact_folder_create(&mut client, &name);

    with_cleanup(
        &mut client,
        |client| {
            let document = vcard_document(&uid, &name, true);
            let written = MsgraphContact::create_from_vcard(&document).expect("the vCard projects");
            let created = client
                .contact_create(Some(&folder_id), &written)
                .expect("contact create")
                .response;

            let fetched = client
                .contact_get(&created.id, Some(MSGRAPH_CONTACT_STASH_EXPAND))
                .expect("contact get")
                .response;
            assert_eq!(
                fetched.stashed_uid().as_deref(),
                Some(uid.as_str()),
                "the UID rides the stash"
            );
            let read = fetched.to_vcard();
            assert!(
                read.contains("X-PIMALAYA-TEST:kept verbatim"),
                "the stash restores the unmanaged lines:\n{read}"
            );
            assert_same_contact(&document, &read);

            let edited = vcard_document(&uid, &format!("{name} renamed"), false);
            let patch = MsgraphContact::update_from_vcard(&edited, &read)
                .expect("the edited vCard projects");
            client
                .contact_update(&created.id, &patch)
                .expect("contact update");

            let refetched = client
                .contact_get(&created.id, Some(MSGRAPH_CONTACT_STASH_EXPAND))
                .expect("contact get after update")
                .response;
            assert_same_contact(&edited, &refetched.to_vcard());
        },
        |client| {
            if let Err(err) = client.contact_folder_delete(&folder_id) {
                report_leftover("contact folder", &folder_id, &err);
            }
        },
    );
}

/// The mail folder surface: get, rename, and a child folder moved and
/// copied in, all inside the test folder.
fn mail_folders(client: &mut MsgraphClientStd, folder_id: &str, name: &str) {
    let fetched = client
        .mail_folder_get(folder_id)
        .expect("mail folder get")
        .response;
    assert_eq!(fetched.display_name, name);

    let renamed = format!("{name} renamed");
    let updated = client
        .mail_folder_update(
            folder_id,
            &MsgraphMailFolder {
                display_name: renamed.clone(),
                ..Default::default()
            },
        )
        .expect("mail folder update")
        .response;
    assert_eq!(updated.display_name, renamed);

    // NOTE: Exchange refuses a copy into the folder already holding the
    // original (ErrorMoveCopyFailed), so a grandchild is copied up one
    // level instead.
    let child = mail_folder_nest(client, &format!("{name} child"), folder_id);
    let grandchild = mail_folder_nest(client, &format!("{name} grandchild"), &child);
    client
        .mail_folder_copy(&grandchild, folder_id)
        .expect("mail folder copy");

    let children = client
        .mail_child_folders_list(folder_id, &Default::default())
        .expect("mail child folders list")
        .response;
    assert_eq!(
        children.value.len(),
        2,
        "the child and the grandchild's copy: {children:?}"
    );
}

/// Creates a mail folder under `parent`, returning its id.
///
/// A folder can only be created at the root, so it is created there and
/// moved in at once; should the move fail, it is deleted rather than
/// left at the root of the mailbox.
fn mail_folder_nest(client: &mut MsgraphClientStd, name: &str, parent: &str) -> String {
    let folder = client
        .mail_folder_create(&MsgraphMailFolder {
            display_name: String::from(name),
            ..Default::default()
        })
        .expect("mail folder create")
        .response;

    match client.mail_folder_move(&folder.id, parent) {
        Ok(moved) => moved.response.id,
        Err(err) => {
            if let Err(err) = client.mail_folder_delete(&folder.id) {
                report_leftover("mail folder", &folder.id, &err);
            }
            panic!("mail folder move: {err:?}");
        }
    }
}

/// The message surface inside the test folder: drafts in JSON and MIME,
/// reads, a patch, attachments, a copy and a move.
fn messages(client: &mut MsgraphClientStd, folder_id: &str, name: &str, address: &str) {
    let subject = format!("{name} draft");
    let draft = MsgraphMessage {
        subject: Some(subject.clone()),
        body: Some(MsgraphItemBody {
            content_type: Some(MsgraphBodyType::Text),
            content: Some(String::from("hello from io-msgraph")),
        }),
        to_recipients: vec![recipient(address)],
        ..Default::default()
    };
    let message = client
        .message_create(Some(folder_id), &draft)
        .expect("message create")
        .response;
    assert_eq!(message.subject.as_deref(), Some(subject.as_str()));

    let fetched = client
        .message_get(&message.id)
        .expect("message get")
        .response;
    assert_eq!(fetched.subject.as_deref(), Some(subject.as_str()));

    let raw = client
        .message_get_raw(&message.id)
        .expect("message get raw")
        .response;
    let raw = String::from_utf8_lossy(&raw);
    assert!(raw.contains(&subject), "the MIME carries the subject");

    let updated = client
        .message_update(
            &message.id,
            &MsgraphMessage {
                is_read: Some(true),
                ..Default::default()
            },
        )
        .expect("message update")
        .response;
    assert_eq!(updated.is_read, Some(true));

    let attachment = client
        .attachment_create(
            &message.id,
            "io-msgraph.txt",
            b"attached by io-msgraph",
            Some("text/plain"),
        )
        .expect("attachment create")
        .response;
    let attachments = client
        .attachments_list(&message.id)
        .expect("attachments list")
        .response;
    assert!(attachments.value.iter().any(|a| a.id == attachment.id));
    let content = client
        .attachment_get_raw(&message.id, &attachment.id)
        .expect("attachment get raw")
        .response;
    assert_eq!(content, b"attached by io-msgraph");
    client
        .attachment_delete(&message.id, &attachment.id)
        .expect("attachment delete");

    let mime_subject = format!("{name} mime");
    let mime = format!(
        "From: {address}\r\nTo: {address}\r\nSubject: {mime_subject}\r\n\r\nimported by io-msgraph\r\n"
    );
    let imported = client
        .message_create_mime(Some(folder_id), mime.as_bytes())
        .expect("message create mime")
        .response;
    assert_eq!(imported.subject.as_deref(), Some(mime_subject.as_str()));

    let copy = client
        .message_copy(&message.id, folder_id)
        .expect("message copy")
        .response;
    let moved = client
        .message_move(&copy.id, "drafts")
        .expect("message move")
        .response;
    client
        .message_move(&moved.id, folder_id)
        .expect("message move back");

    let listed = client
        .messages_list(
            Some(folder_id),
            &MsgraphMessagesListParams {
                top: Some(10),
                ..Default::default()
            },
        )
        .expect("messages list")
        .response;
    assert_eq!(
        listed.value.len(),
        3,
        "the draft, the MIME import and the copy"
    );

    client.message_delete(&imported.id).expect("message delete");
}

/// One delta round over the test folder, then a change caught by the
/// next round from its delta link.
fn messages_delta(client: &mut MsgraphClientStd, folder_id: &str, name: &str) {
    let mut page = client
        .messages_delta(Some(folder_id), None)
        .expect("messages delta")
        .response;
    while let Some(next) = page.next_link.clone() {
        page = client
            .messages_delta_from_link(&next)
            .expect("messages delta page")
            .response;
    }
    let delta_link = page.delta_link.expect("a delta round ends on a delta link");

    let subject = format!("{name} delta");
    let created = client
        .message_create(
            Some(folder_id),
            &MsgraphMessage {
                subject: Some(subject),
                ..Default::default()
            },
        )
        .expect("message create for delta")
        .response;

    eventually("the delta lists the new message", || {
        let page = client
            .messages_delta_from_link(&delta_link)
            .expect("messages delta from link")
            .response;
        page.value
            .iter()
            .any(|delta| delta.message.id == created.id)
    });
}

/// The error path and the batch: a message read after its delete
/// answers the Graph error envelope, alone and inside a batch beside a
/// request that succeeds.
fn errors_and_batch(client: &mut MsgraphClientStd, folder_id: &str, name: &str, address: &str) {
    let message = client
        .message_create(
            Some(folder_id),
            &MsgraphMessage {
                subject: Some(format!("{name} gone")),
                ..Default::default()
            },
        )
        .expect("message create for the error path")
        .response;
    client
        .message_delete(&message.id)
        .expect("message delete for the error path");

    match client.message_get(&message.id) {
        Err(MsgraphClientStdError::Send(err)) => {
            assert_eq!(err.status(), Some(404), "a deleted message is gone: {err}");
            assert!(!err.is_retryable(), "a 404 is no transient failure");
        }
        other => panic!("a deleted message should answer 404, got {other:?}"),
    }

    let requests = [
        MsgraphBatchRequest {
            id: String::from("folder"),
            method: String::from("GET"),
            url: format!("/users/{address}/mailFolders/{folder_id}"),
            ..Default::default()
        },
        MsgraphBatchRequest {
            id: String::from("gone"),
            method: String::from("GET"),
            url: format!("/users/{address}/messages/{}", message.id),
            ..Default::default()
        },
    ];
    let responses = client.batch(&requests).expect("batch").response.responses;
    assert_eq!(responses.len(), 2, "one response per request");

    for response in responses {
        match response.id.as_str() {
            "folder" => {
                let folder = response
                    .parse::<MsgraphMailFolder>()
                    .expect("the folder request succeeds");
                assert_eq!(folder.id, folder_id);
            }
            "gone" => {
                let err = response
                    .parse::<MsgraphMessage>()
                    .expect_err("the deleted message fails inside the batch");
                assert_eq!(err.status(), Some(404), "{err}");
            }
            id => panic!("unexpected batch response id `{id}`"),
        }
    }
}

/// Sends to the mailbox itself three ways (a draft, JSON, MIME), then
/// waits for the three deliveries.
fn send(client: &mut MsgraphClientStd, address: &str, subject: &str) {
    let draft = client
        .message_create(
            None,
            &MsgraphMessage {
                subject: Some(String::from(subject)),
                to_recipients: vec![recipient(address)],
                ..Default::default()
            },
        )
        .expect("draft create")
        .response;
    client.message_send(&draft.id).expect("message send");

    client
        .mail_send(
            &MsgraphMessage {
                subject: Some(String::from(subject)),
                body: Some(MsgraphItemBody {
                    content_type: Some(MsgraphBodyType::Text),
                    content: Some(String::from("sent by io-msgraph as JSON")),
                }),
                to_recipients: vec![recipient(address)],
                ..Default::default()
            },
            true,
        )
        .expect("mail send");

    let mime = format!("To: {address}\r\nSubject: {subject}\r\n\r\nsent by io-msgraph as MIME\r\n");
    client
        .mail_send_mime(mime.as_bytes())
        .expect("mail send mime");

    eventually("the three messages reach the inbox", || {
        with_subject(client, "inbox", subject).len() == 3
    });
}

/// Deletes what [`send`] left in the inbox and the sent items.
fn sweep(client: &mut MsgraphClientStd, subject: &str) {
    for folder in ["inbox", "sentitems"] {
        for id in with_subject(client, folder, subject) {
            if let Err(err) = client.message_delete(&id) {
                report_leftover("message", &id, &err);
            }
        }
    }
}

/// The ids of the messages of a well-known folder bearing `subject`.
fn with_subject(client: &mut MsgraphClientStd, folder: &str, subject: &str) -> Vec<String> {
    let filter = format!("subject eq '{subject}'");
    let params = MsgraphMessagesListParams {
        top: Some(25),
        filter: Some(&filter),
        select: Some("id"),
        ..Default::default()
    };

    match client.messages_list(Some(folder), &params) {
        Ok(listed) => listed.response.value.into_iter().map(|m| m.id).collect(),
        Err(err) => {
            eprintln!("WARNING: could not list {folder} for `{subject}`: {err:?}");
            Vec::new()
        }
    }
}

fn contact_folder_create(client: &mut MsgraphClientStd, name: &str) -> String {
    let folder = client
        .contact_folder_create(&MsgraphContactFolder {
            display_name: String::from(name),
            ..Default::default()
        })
        .expect("contact folder create")
        .response;
    assert_eq!(folder.display_name, name);
    folder.id
}

/// The contact folder surface: get, rename, and the empty child list of
/// a fresh folder.
fn contact_folders(client: &mut MsgraphClientStd, folder_id: &str, name: &str) {
    let fetched = client
        .contact_folder_get(folder_id)
        .expect("contact folder get")
        .response;
    assert_eq!(fetched.display_name, name);

    let renamed = format!("{name} renamed");
    let updated = client
        .contact_folder_update(
            folder_id,
            &MsgraphContactFolder {
                display_name: renamed.clone(),
                ..Default::default()
            },
        )
        .expect("contact folder update")
        .response;
    assert_eq!(updated.display_name, renamed);

    let children = client
        .contact_child_folders_list(folder_id, &Default::default())
        .expect("contact child folders list")
        .response;
    assert!(children.value.is_empty(), "a fresh folder has no child");
}

/// Contact create, get, patch, list and delete inside the test folder.
fn contact_crud(client: &mut MsgraphClientStd, folder_id: &str, name: &str) {
    let contact = client
        .contact_create(
            Some(folder_id),
            &MsgraphContact {
                given_name: MsgraphField::Set(String::from(name)),
                surname: MsgraphField::Set(String::from("Test")),
                email_addresses: MsgraphField::Set(vec![MsgraphEmailAddress {
                    name: Some(String::from(name)),
                    address: Some(String::from("io-msgraph-test@example.com")),
                }]),
                business_phones: MsgraphField::Set(vec![String::from("+1 234 567 890")]),
                ..Default::default()
            },
        )
        .expect("contact create")
        .response;
    assert_eq!(contact.given_name.as_deref(), Some(name));

    let fetched = client
        .contact_get(&contact.id, None)
        .expect("contact get")
        .response;
    let emails = fetched
        .email_addresses
        .as_option()
        .expect("email addresses");
    assert_eq!(
        emails[0].address.as_deref(),
        Some("io-msgraph-test@example.com")
    );

    let updated = client
        .contact_update(
            &contact.id,
            &MsgraphContact {
                surname: MsgraphField::Set(String::from("Renamed")),
                ..Default::default()
            },
        )
        .expect("contact update")
        .response;
    assert_eq!(updated.surname.as_deref(), Some("Renamed"));

    let listed = client
        .contacts_list(
            Some(folder_id),
            &MsgraphContactsListParams {
                top: Some(10),
                ..Default::default()
            },
        )
        .expect("contacts list")
        .response;
    assert!(listed.value.iter().any(|c| c.id == contact.id));

    client.contact_delete(&contact.id).expect("contact delete");
}

/// One delta round over the test folder, then a create caught by the
/// next round from its delta link.
fn contacts_delta(client: &mut MsgraphClientStd, folder_id: &str, name: &str) {
    let mut page = client
        .contacts_delta(Some(folder_id), None)
        .expect("contacts delta")
        .response;
    while let Some(next) = page.next_link.clone() {
        page = client
            .contacts_delta_from_link(&next)
            .expect("contacts delta page")
            .response;
    }
    let delta_link = page.delta_link.expect("a delta round ends on a delta link");

    let created = client
        .contact_create(
            Some(folder_id),
            &MsgraphContact {
                given_name: MsgraphField::Set(format!("{name} delta")),
                ..Default::default()
            },
        )
        .expect("contact create for delta")
        .response;

    eventually("the delta lists the new contact", || {
        let page = client
            .contacts_delta_from_link(&delta_link)
            .expect("contacts delta from link")
            .response;
        page.value
            .iter()
            .any(|delta| delta.contact.id == created.id)
    });
}

fn calendar_create(client: &mut MsgraphClientStd, name: &str) -> String {
    let calendar = client
        .calendar_create(&MsgraphCalendar {
            name: MsgraphField::Set(String::from(name)),
            ..Default::default()
        })
        .expect("calendar create")
        .response;
    assert_eq!(calendar.name.as_deref(), Some(name));
    calendar.id
}

/// The calendar surface: get, rename and recolour, list.
fn calendar_metadata(client: &mut MsgraphClientStd, calendar_id: &str, name: &str) {
    let fetched = client
        .calendar_get(calendar_id)
        .expect("calendar get")
        .response;
    assert_eq!(fetched.name.as_deref(), Some(name));

    let renamed = format!("{name} renamed");
    let updated = client
        .calendar_update(
            calendar_id,
            &MsgraphCalendar {
                name: MsgraphField::Set(renamed.clone()),
                color: MsgraphField::Set(String::from("lightGreen")),
                ..Default::default()
            },
        )
        .expect("calendar update")
        .response;
    assert_eq!(updated.name.as_deref(), Some(renamed.as_str()));
    assert_eq!(updated.color.as_deref(), Some("lightGreen"));

    let listed = client
        .calendars_list(&Default::default())
        .expect("calendars list")
        .response;
    assert!(listed.value.iter().any(|c| c.id == calendar_id));
}

/// A lone event: create, get, patch, then found by the listing and the
/// calendar view, and deleted.
fn single_event(client: &mut MsgraphClientStd, calendar_id: &str, name: &str) {
    let subject = format!("{name} single");
    let event = client
        .event_create(
            Some(calendar_id),
            &MsgraphEvent {
                subject: MsgraphField::Set(subject.clone()),
                start: MsgraphField::Set(at("2030-01-07T10:00:00")),
                end: MsgraphField::Set(at("2030-01-07T11:00:00")),
                ..Default::default()
            },
        )
        .expect("event create")
        .response;
    assert_eq!(event.subject.as_deref(), Some(subject.as_str()));

    let fetched = client
        .event_get(&event.id, None, None)
        .expect("event get")
        .response;
    assert_eq!(
        fetched.start,
        MsgraphField::Set(at("2030-01-07T10:00:00.0000000"))
    );

    let renamed = format!("{subject} renamed");
    let updated = client
        .event_update(
            &event.id,
            &MsgraphEvent {
                subject: MsgraphField::Set(renamed.clone()),
                ..Default::default()
            },
        )
        .expect("event update")
        .response;
    assert_eq!(updated.subject.as_deref(), Some(renamed.as_str()));
    assert_ne!(
        updated.change_key, fetched.change_key,
        "a patch moves the changeKey"
    );

    let listed = client
        .events_list(Some(calendar_id), &Default::default())
        .expect("events list")
        .response;
    assert!(listed.value.iter().any(|e| e.id == event.id));

    let view = client
        .calendar_view(
            Some(calendar_id),
            WINDOW_START,
            WINDOW_END,
            &Default::default(),
        )
        .expect("calendar view")
        .response;
    assert!(view.value.iter().any(|e| e.id == event.id));

    client.event_delete(&event.id).expect("event delete");
}

/// A listing one event per page, followed through its next link until
/// both events of the calendar were seen.
fn events_paging(client: &mut MsgraphClientStd, calendar_id: &str, name: &str) {
    let mut created = Vec::new();
    for (index, day) in ["2030-01-14", "2030-01-15"].iter().enumerate() {
        let event = client
            .event_create(
                Some(calendar_id),
                &MsgraphEvent {
                    subject: MsgraphField::Set(format!("{name} page {index}")),
                    start: MsgraphField::Set(at(&format!("{day}T10:00:00"))),
                    end: MsgraphField::Set(at(&format!("{day}T11:00:00"))),
                    ..Default::default()
                },
            )
            .expect("event create for paging")
            .response;
        created.push(event.id);
    }

    let params = MsgraphEventsListParams {
        top: Some(1),
        ..Default::default()
    };
    let mut page = client
        .events_list(Some(calendar_id), &params)
        .expect("events list, first page")
        .response;
    assert_eq!(page.value.len(), 1, "one event per page");

    let mut seen: Vec<String> = page.value.drain(..).map(|event| event.id).collect();
    while let Some(next) = page.next_link.take() {
        page = client
            .events_list_from_link(&next)
            .expect("events list, next page")
            .response;
        seen.extend(page.value.drain(..).map(|event| event.id));
    }

    for id in &created {
        assert!(seen.contains(id), "the pages miss event `{id}`");
        client.event_delete(id).expect("event delete after paging");
    }
}

/// A weekly series of three: its instances, one modified and one
/// cancelled, as the master and the view then report them.
fn recurring_event(client: &mut MsgraphClientStd, calendar_id: &str, name: &str) {
    let subject = format!("{name} weekly");
    let master = client
        .event_create(
            Some(calendar_id),
            &MsgraphEvent {
                subject: MsgraphField::Set(subject.clone()),
                start: MsgraphField::Set(at("2030-01-10T10:00:00")),
                end: MsgraphField::Set(at("2030-01-10T11:00:00")),
                recurrence: MsgraphField::Set(weekly_thrice("2030-01-10")),
                ..Default::default()
            },
        )
        .expect("recurring event create")
        .response;

    let instances = client
        .event_instances(&master.id, WINDOW_START, WINDOW_END, &Default::default())
        .expect("event instances")
        .response;
    assert_eq!(instances.value.len(), 3, "three weekly instances");
    // NOTE: the iCalendar projection tells an exception's UTC times in its
    // creation zone, so an unselected listing has to carry it.
    assert!(
        instances
            .value
            .iter()
            .all(|instance| instance.original_start_time_zone.is_some()),
        "instances carry their original zone by default: {instances:?}"
    );

    let exception = format!("{subject} moved");
    client
        .event_update(
            &instances.value[1].id,
            &MsgraphEvent {
                subject: MsgraphField::Set(exception.clone()),
                ..Default::default()
            },
        )
        .expect("instance update");
    client
        .event_delete(&instances.value[2].id)
        .expect("instance delete");

    let instances = client
        .event_instances(&master.id, WINDOW_START, WINDOW_END, &Default::default())
        .expect("event instances after edits")
        .response;
    assert_eq!(instances.value.len(), 2, "the cancelled instance is gone");
    assert_eq!(
        instances
            .value
            .iter()
            .filter(|e| e.event_type == Some(MsgraphEventType::Exception))
            .count(),
        1,
        "the modified instance turned into an exception"
    );
    assert!(
        instances
            .value
            .iter()
            .any(|e| e.subject.as_deref() == Some(exception.as_str())),
        "the modified instance keeps its own subject"
    );

    let fetched = client
        .event_get(&master.id, Some("cancelledOccurrences,recurrence"), None)
        .expect("master get")
        .response;
    assert_eq!(
        fetched.cancelled_occurrences.len(),
        1,
        "the master records the cancelled occurrence"
    );

    client
        .event_delete(&master.id)
        .expect("recurring event delete");
}

/// One delta round over the calendar window, then a create caught by
/// the next round from its delta link.
fn events_delta(client: &mut MsgraphClientStd, calendar_id: &str, name: &str) {
    let mut page = client
        .events_delta(Some(calendar_id), WINDOW_START, WINDOW_END)
        .expect("events delta")
        .response;
    while let Some(next) = page.next_link.clone() {
        page = client
            .events_delta_from_link(&next)
            .expect("events delta page")
            .response;
    }
    let delta_link = page.delta_link.expect("a delta round ends on a delta link");

    let created = client
        .event_create(
            Some(calendar_id),
            &MsgraphEvent {
                subject: MsgraphField::Set(format!("{name} delta")),
                start: MsgraphField::Set(at("2030-01-20T10:00:00")),
                end: MsgraphField::Set(at("2030-01-20T11:00:00")),
                ..Default::default()
            },
        )
        .expect("event create for delta")
        .response;

    eventually("the delta lists the new event", || {
        let page = client
            .events_delta_from_link(&delta_link)
            .expect("events delta from link")
            .response;
        page.value.iter().any(|delta| delta.event.id == created.id)
    });
}

fn todo_list_create(client: &mut MsgraphClientStd, name: &str) -> String {
    let list = client
        .todo_list_create(&MsgraphTodoList {
            display_name: MsgraphField::Set(name.to_owned()),
            ..Default::default()
        })
        .expect("todo list create")
        .response;
    assert_eq!(list.display_name.as_deref(), Some(name));
    assert_eq!(
        list.wellknown_list_name,
        Some(MsgraphTodoWellknownListName::None)
    );
    list.id
}

/// The list read back, renamed, then found by the listing and by a lists
/// delta round.
fn todo_list_metadata(client: &mut MsgraphClientStd, list_id: &str, name: &str) {
    let fetched = client
        .todo_list_get(list_id)
        .expect("todo list get")
        .response;
    assert_eq!(fetched.display_name.as_deref(), Some(name));
    assert!(fetched.etag.is_some(), "a list carries its etag");

    let renamed = format!("{name} renamed");
    let updated = client
        .todo_list_update(
            list_id,
            &MsgraphTodoList {
                display_name: MsgraphField::Set(renamed.clone()),
                ..Default::default()
            },
        )
        .expect("todo list update")
        .response;
    assert_eq!(updated.display_name.as_deref(), Some(renamed.as_str()));

    let mut listed = client
        .todo_lists_list(&Default::default())
        .expect("todo lists list")
        .response;
    let mut found = listed.value.iter().any(|list| list.id == list_id);
    while let (false, Some(next)) = (found, listed.next_link.clone()) {
        listed = client
            .todo_lists_list_from_link(&next)
            .expect("todo lists list page")
            .response;
        found = listed.value.iter().any(|list| list.id == list_id);
    }
    assert!(found, "the listing holds the list");

    let mut page = client
        .todo_lists_delta()
        .expect("todo lists delta")
        .response;
    let mut found = page.value.iter().any(|delta| delta.list.id == list_id);
    while let Some(next) = page.next_link.clone() {
        page = client
            .todo_lists_delta_from_link(&next)
            .expect("todo lists delta page")
            .response;
        found |= page.value.iter().any(|delta| delta.list.id == list_id);
    }
    assert!(found, "the first delta round holds the list");
    assert!(
        page.delta_link.is_some(),
        "a delta round ends on a delta link"
    );
}

/// A task with every field To Do keeps: created, read back, patched
/// (clearing its due date), listed and deleted.
fn todo_task_crud(client: &mut MsgraphClientStd, list_id: &str, name: &str) {
    let title = format!("{name} task");
    let created = client
        .todo_task_create(
            list_id,
            &MsgraphTodoTask {
                title: MsgraphField::Set(title.clone()),
                categories: MsgraphField::Set(vec!["inbox".into(), "@phone".into()]),
                importance: MsgraphField::Set(MsgraphImportance::High),
                status: MsgraphField::Set(MsgraphTodoTaskStatus::WaitingOnOthers),
                due_date_time: MsgraphField::Set(MsgraphDateTimeTimeZone {
                    date_time: "2030-01-15T00:00:00".into(),
                    time_zone: Some("Europe/Paris".into()),
                }),
                linked_resources: vec![MsgraphLinkedResource {
                    web_url: Some("mid:abc@example.com".into()),
                    application_name: Some("io-msgraph".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .expect("todo task create")
        .response;
    assert!(created.etag.is_some(), "a task carries its etag");
    assert_eq!(
        created.categories.as_option().map(Vec::as_slice),
        Some(&["inbox".to_owned(), "@phone".to_owned()][..]),
        "categories take any string"
    );
    assert_eq!(
        created.linked_resources[0].web_url.as_deref(),
        Some("mid:abc@example.com"),
        "a linked resource keeps a mid: URI"
    );

    let fetched = client
        .todo_task_get(list_id, &created.id, None, Some("checklistItems"))
        .expect("todo task get")
        .response;
    assert_eq!(fetched.title.as_deref(), Some(title.as_str()));
    assert_eq!(
        fetched.status,
        MsgraphField::Set(MsgraphTodoTaskStatus::WaitingOnOthers)
    );

    let updated = client
        .todo_task_update(
            list_id,
            &created.id,
            &MsgraphTodoTask {
                title: MsgraphField::Set(format!("{title} renamed")),
                due_date_time: MsgraphField::Null,
                ..Default::default()
            },
        )
        .expect("todo task update")
        .response;
    assert_eq!(
        updated.due_date_time,
        MsgraphField::Unset,
        "the due date is cleared"
    );
    assert_ne!(updated.etag, created.etag, "an edit moves the etag");

    let listed = client
        .todo_tasks_list(list_id, &Default::default())
        .expect("todo tasks list")
        .response;
    assert!(listed.value.iter().any(|task| task.id == created.id));

    let items = client
        .checklist_items_list(list_id, &created.id)
        .expect("checklist items list")
        .response;
    assert!(items.value.is_empty());

    client
        .todo_task_delete(list_id, &created.id)
        .expect("todo task delete");
    let err = client
        .todo_task_get(list_id, &created.id, None, None)
        .expect_err("a deleted task is gone");
    assert!(
        matches!(&err, MsgraphClientStdError::Send(err) if err.status() == Some(404)),
        "got: {err:?}"
    );
}

/// A tasks delta round, then the next one listing a created task and its
/// removal.
fn todo_tasks_delta(client: &mut MsgraphClientStd, list_id: &str, name: &str) {
    let mut page = client
        .todo_tasks_delta(list_id, None)
        .expect("todo tasks delta")
        .response;
    while let Some(next) = page.next_link.clone() {
        page = client
            .todo_tasks_delta_from_link(&next)
            .expect("todo tasks delta page")
            .response;
    }
    let delta_link = page.delta_link.expect("a delta round ends on a delta link");

    let created = client
        .todo_task_create(
            list_id,
            &MsgraphTodoTask {
                title: MsgraphField::Set(format!("{name} delta")),
                ..Default::default()
            },
        )
        .expect("todo task create for delta")
        .response;

    let mut next_round = None;
    eventually("the delta lists the new task", || {
        let page = client
            .todo_tasks_delta_from_link(&delta_link)
            .expect("todo tasks delta from link")
            .response;
        next_round = page.delta_link.clone();
        page.value.iter().any(|delta| delta.task.id == created.id)
    });

    client
        .todo_task_delete(list_id, &created.id)
        .expect("todo task delete for delta");
    let next_round = next_round.expect("the round ends on a delta link");

    eventually("the delta lists the removal", || {
        let page = client
            .todo_tasks_delta_from_link(&next_round)
            .expect("todo tasks delta after the removal")
            .response;
        page.value.iter().any(|delta| {
            delta.task.id == created.id
                && delta
                    .removed
                    .as_ref()
                    .is_some_and(|removed| removed.reason == "deleted")
        })
    });
}

/// A one-task document holding `lines` beside its UID.
#[cfg(feature = "ical")]
fn todo_document(uid: &str, lines: &[&str]) -> String {
    let mut document = vec![
        String::from("BEGIN:VCALENDAR"),
        String::from("VERSION:2.0"),
        String::from("PRODID:-//Pimalaya//io-msgraph tests//EN"),
        String::from("BEGIN:VTODO"),
        format!("UID:{uid}"),
        String::from("DTSTAMP:20260101T000000Z"),
    ];
    document.extend(lines.iter().map(|line| String::from(*line)));
    document.push(String::from("END:VTODO"));
    document.push(String::from("END:VCALENDAR"));
    document.push(String::new());
    document.join("\r\n")
}

/// The VTODO lines of a document, Graph's stamps aside, sorted.
#[cfg(feature = "ical")]
fn todo_lines(document: &str) -> Vec<&str> {
    let start = document.find("BEGIN:VTODO").expect("a VTODO");
    let end = document.find("END:VTODO").expect("an END:VTODO");
    let mut lines: Vec<&str> = document[start..end]
        .split("\r\n")
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !["DTSTAMP", "CREATED", "LAST-MODIFIED"]
                .iter()
                .any(|stamp| line.starts_with(stamp))
        })
        .collect();
    lines.sort_unstable();
    lines
}

/// A one-event document with the given timing lines (DTSTART, DTEND,
/// RRULE, ATTENDEE…), the rest kept minimal.
#[cfg(feature = "ical")]
fn event_document(uid: &str, summary: &str, timing: &[&str]) -> String {
    let mut lines = vec![
        String::from("BEGIN:VCALENDAR"),
        String::from("VERSION:2.0"),
        String::from("PRODID:-//pimalaya//io-msgraph tests//EN"),
        String::from("BEGIN:VEVENT"),
        format!("UID:{uid}"),
        String::from("DTSTAMP:20300101T000000Z"),
        format!("SUMMARY:{summary}"),
        String::from("DESCRIPTION:written by the io-msgraph suite"),
        String::from("TRANSP:OPAQUE"),
        String::from("CLASS:PUBLIC"),
    ];
    lines.extend(timing.iter().map(|line| String::from(*line)));
    lines.extend([
        String::from("END:VEVENT"),
        String::from("END:VCALENDAR"),
        String::new(),
    ]);
    lines.join("\r\n")
}

#[cfg(feature = "ical")]
fn ical_document(uid: &str, summary: &str, location: bool) -> String {
    let mut lines = vec![
        String::from("BEGIN:VCALENDAR"),
        String::from("VERSION:2.0"),
        String::from("PRODID:-//pimalaya//io-msgraph tests//EN"),
        String::from("BEGIN:VEVENT"),
        format!("UID:{uid}"),
        String::from("DTSTAMP:20300101T000000Z"),
        String::from("DTSTART;TZID=Europe/Paris:20300110T100000"),
        String::from("DTEND;TZID=Europe/Paris:20300110T110000"),
        String::from("RRULE:FREQ=WEEKLY;COUNT=2"),
        format!("SUMMARY:{summary}"),
        String::from("DESCRIPTION:written by the io-msgraph suite"),
        String::from("TRANSP:TRANSPARENT"),
        String::from("CLASS:PRIVATE"),
        String::from("CATEGORIES:io-msgraph"),
        String::from("X-PIMALAYA-TEST:kept verbatim"),
    ];

    if location {
        lines.push(String::from("LOCATION:Paris"));
    }

    lines.extend([
        String::from("BEGIN:VALARM"),
        String::from("ACTION:DISPLAY"),
        String::from("DESCRIPTION:reminder"),
        String::from("TRIGGER:-PT15M"),
        String::from("END:VALARM"),
        String::from("END:VEVENT"),
        String::from("END:VCALENDAR"),
        String::new(),
    ]);

    lines.join("\r\n")
}

/// The value of the event's DESCRIPTION line, the alarm's excluded.
#[cfg(feature = "ical")]
fn description(document: &str) -> Option<&str> {
    document
        .split("BEGIN:VALARM")
        .next()?
        .lines()
        .find_map(|line| line.strip_prefix("DESCRIPTION:"))
        .map(str::trim_end)
}

/// Asserts that the document read back projects like the one written,
/// on every field the projection manages.
#[cfg(feature = "ical")]
fn assert_same_event(written_doc: &str, read_doc: &str) {
    let written = MsgraphEvent::from_ical(written_doc.as_bytes()).expect("written projects");
    let read = MsgraphEvent::from_ical(read_doc.as_bytes()).expect("read projects");
    let context = format!("written:\n{written_doc}\nread:\n{read_doc}");

    assert_eq!(read.subject, written.subject, "SUMMARY altered\n{context}");
    // NOTE: Exchange stores a plain-text body as HTML, so the read
    // document carries an X-ALT-DESC beside the text, and only the text
    // can be compared.
    assert_eq!(
        description(read_doc),
        description(written_doc),
        "DESCRIPTION altered\n{context}"
    );
    assert_eq!(read.start, written.start, "DTSTART altered\n{context}");
    assert_eq!(read.end, written.end, "DTEND altered\n{context}");
    assert_eq!(
        read.is_all_day, written.is_all_day,
        "all-day altered\n{context}"
    );
    assert_eq!(
        read.location, written.location,
        "LOCATION altered\n{context}"
    );
    // NOTE: Exchange states the week start a rule left implicit, as
    // Sunday (WKST=SU), which changes nothing for a weekly rule on one
    // day; only that is forgiven.
    let mut recurrence = read.recurrence.clone();
    if let (MsgraphField::Set(read), MsgraphField::Set(written)) =
        (&mut recurrence, &written.recurrence)
        && written.pattern.first_day_of_week.is_none()
        && read.pattern.first_day_of_week == Some(MsgraphDayOfWeek::Sunday)
    {
        read.pattern.first_day_of_week = None;
    }
    assert_eq!(recurrence, written.recurrence, "RRULE altered\n{context}");
    assert_eq!(
        read.categories, written.categories,
        "CATEGORIES altered\n{context}"
    );
    assert_eq!(read.show_as, written.show_as, "TRANSP altered\n{context}");
    assert_eq!(
        read.sensitivity, written.sensitivity,
        "CLASS altered\n{context}"
    );
    assert_eq!(
        read.is_reminder_on, written.is_reminder_on,
        "VALARM altered\n{context}"
    );
    assert_eq!(
        read.reminder_minutes_before_start, written.reminder_minutes_before_start,
        "VALARM trigger altered\n{context}"
    );
    assert_eq!(
        read.single_value_extended_properties, written.single_value_extended_properties,
        "stash altered\n{context}"
    );
}

#[cfg(feature = "vcard")]
fn vcard_document(uid: &str, name: &str, note: bool) -> String {
    let mut lines = vec![
        String::from("BEGIN:VCARD"),
        String::from("VERSION:4.0"),
        format!("UID:{uid}"),
        format!("FN:{name} Test"),
        format!("N:Test;{name};;;"),
        String::from("EMAIL;TYPE=work:io-msgraph-test@example.com"),
        String::from("TEL;TYPE=cell:+1 234 567 890"),
        String::from("ORG:Pimalaya"),
        String::from("BDAY:19700101"),
        String::from("X-PIMALAYA-TEST:kept verbatim"),
    ];

    if note {
        lines.push(String::from("NOTE:written by the io-msgraph suite"));
    }

    lines.extend([String::from("END:VCARD"), String::new()]);
    lines.join("\r\n")
}

/// Asserts that the vCard read back projects like the one written.
#[cfg(feature = "vcard")]
fn assert_same_contact(written_doc: &str, read_doc: &str) {
    let written = MsgraphContact::from_vcard(written_doc).expect("written projects");
    let read = MsgraphContact::from_vcard(read_doc).expect("read projects");

    assert_eq!(read, written, "written:\n{written_doc}\nread:\n{read_doc}");
}

/// A UTC boundary, as Graph spells it without a zone suffix.
fn at(date_time: &str) -> MsgraphDateTimeTimeZone {
    MsgraphDateTimeTimeZone {
        date_time: String::from(date_time),
        time_zone: Some(String::from("UTC")),
    }
}

/// Every Thursday for three weeks from `start`, a Thursday.
fn weekly_thrice(start: &str) -> MsgraphPatternedRecurrence {
    MsgraphPatternedRecurrence {
        pattern: MsgraphRecurrencePattern {
            pattern_type: Some(MsgraphRecurrencePatternType::Weekly),
            interval: Some(1),
            days_of_week: vec![MsgraphDayOfWeek::Thursday],
            ..Default::default()
        },
        range: MsgraphRecurrenceRange {
            range_type: Some(MsgraphRecurrenceRangeType::Numbered),
            start_date: Some(String::from(start)),
            number_of_occurrences: Some(3),
            recurrence_time_zone: Some(String::from("UTC")),
            ..Default::default()
        },
    }
}

fn recipient(address: &str) -> MsgraphRecipient {
    MsgraphRecipient {
        email_address: MsgraphEmailAddress {
            name: None,
            address: Some(String::from(address)),
        },
    }
}

/// The address of the mailbox the run acts on.
fn address(client: &mut MsgraphClientStd) -> String {
    let me = client.me().expect("me").response;
    me.mail
        .or(me.user_principal_name)
        .expect("the mailbox exposes an address")
}

/// Opens the connection with the environment's token, on the mailbox
/// the run acts on.
fn connect() -> MsgraphClientStd {
    env_logger::try_init().ok();

    let (token, default_user) = token();
    let options = MsgraphClientStdConnectOptions {
        user_id: env::var("MSGRAPH_USER_ID").unwrap_or_else(|_| String::from(default_user)),
        ..Default::default()
    };

    MsgraphClientStd::connect(token, options).expect("connect")
}

/// The access token, and the mailbox it acts on unless
/// `MSGRAPH_USER_ID` says otherwise: `me` for a delegated token minted
/// by hand, the test mailbox for an app-only one minted here.
fn token() -> (String, &'static str) {
    if let Ok(token) = env::var("MSGRAPH_ACCESS_TOKEN") {
        return (token, "me");
    }

    let var = |name: &str| {
        env::var(name).unwrap_or_else(|_| {
            panic!(
                "set MSGRAPH_ACCESS_TOKEN, or MSGRAPH_TENANT_ID, MSGRAPH_CLIENT_ID and \
                 MSGRAPH_CLIENT_SECRET to mint one ({name} is missing)"
            )
        })
    };

    let tenant = var("MSGRAPH_TENANT_ID");
    let client_id = var("MSGRAPH_CLIENT_ID");
    let secret = var("MSGRAPH_CLIENT_SECRET");

    (mint_token(&tenant, &client_id, secret), DEFAULT_USER)
}

/// Trades the app's client secret for an app-only Graph token (RFC
/// 6749 section 4.4).
fn mint_token(tenant: &str, client_id: &str, secret: String) -> String {
    let token_uri: Url = format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token")
        .parse()
        .expect("the token URI is a valid URL");

    let mut client = Oauth20ClientStd::connect(token_uri, &Tls::default(), client_id)
        .expect("connect to the token endpoint");
    client.client_secret = Some(SecretString::from(secret));

    let params = Oauth20ClientCredentialsRequestParams {
        scope: [Cow::from(GRAPH_SCOPE)].into_iter().collect(),
    };

    match client
        .request_client_credentials(params)
        .expect("request the client credentials grant")
    {
        Ok(granted) => granted.access_token.expose_secret().to_owned(),
        Err(err) => panic!("the token endpoint refused the client: {err:?}"),
    }
}

/// Milliseconds since the Unix epoch, to name a run's resources.
fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

/// Polls `check` until it holds, panicking with `what` after
/// [`SETTLE`].
///
/// Graph acknowledges a write before every read reflects it: a delivery,
/// a delta round, a listing all catch up within seconds, sometimes more.
fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + SETTLE;

    loop {
        if check() {
            return;
        }
        if Instant::now() > deadline {
            panic!("timed out waiting until {what}");
        }
        thread::sleep(Duration::from_secs(3));
    }
}

/// Runs `body`, then `cleanup` whichever way `body` went, and only then
/// re-raises a panic `body` may have raised.
///
/// These flows run against a real mailbox. Every step panics on failure,
/// so a teardown written as the last statements of a flow would be
/// skipped the moment anything goes wrong, leaving the folder or the
/// calendar behind for good.
fn with_cleanup<B, C>(client: &mut MsgraphClientStd, body: B, cleanup: C)
where
    B: FnOnce(&mut MsgraphClientStd),
    C: FnOnce(&mut MsgraphClientStd),
{
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| body(client)));

    if panic::catch_unwind(AssertUnwindSafe(|| cleanup(client))).is_err() {
        eprintln!("WARNING: cleanup itself failed, the mailbox may hold leftovers");
    }

    if let Err(payload) = outcome {
        panic::resume_unwind(payload);
    }
}

/// Reports a failed teardown without panicking, naming what was left
/// behind so it can be removed by hand.
fn report_leftover(what: &str, id: &str, err: &dyn Debug) {
    eprintln!("WARNING: could not clean up {what} `{id}`, remove it by hand: {err:?}");
}
