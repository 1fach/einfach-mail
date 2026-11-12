use einfach_mail_domain::{CalendarEventCreationId, EventId};
use ews::response::ResponseCode;

use crate::transport::serialize_soap_request;

use super::{
    CalendarItemSnapshotError, OPERATION_MARKER_PROPERTY_NAME, OPERATION_MARKER_PROPERTY_SET_ID,
    get_item_request, parse_calendar_item_snapshot,
};

const SUPPORTED_ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-supported.xml");
const WRONG_KIND_ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-wrong-kind.xml");
const REJECTED_ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-rejected.xml");
const MALFORMED_ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-malformed.xml");
const INCOMPLETE_ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-incomplete.xml");
const NAMESPACED_ATTRIBUTES_ITEM: &[u8] =
    include_bytes!("../../../fixtures/calendar-item-namespaced-attributes.xml");
const CONTRADICTORY_RECURRENCE_ITEM: &[u8] =
    include_bytes!("../../../fixtures/calendar-item-contradictory-recurrence.xml");
const NIL_RECURRENCE_ITEM: &[u8] =
    include_bytes!("../../../fixtures/calendar-item-nil-recurrence.xml");

fn expected_id() -> EventId {
    EventId::try_from("synthetic-calendar-event".to_owned()).expect("synthetic event ID is valid")
}

fn appointment_xml() -> String {
    String::from_utf8(SUPPORTED_ITEM.to_vec()).expect("synthetic XML is UTF-8")
}

fn replace_once(document: &str, from: &str, to: &str) -> String {
    assert_eq!(document.matches(from).count(), 1);
    document.replacen(from, to, 1)
}

fn assert_ineligible(document: &str) {
    assert_eq!(
        parse_calendar_item_snapshot(document.as_bytes(), &expected_id()),
        Err(CalendarItemSnapshotError::Ineligible)
    );
}

#[test]
fn get_item_request_selects_the_supported_shape_and_stable_marker_property() {
    let request = get_item_request(&expected_id());
    let xml = String::from_utf8(
        serialize_soap_request(request).expect("synthetic GetItem request serializes"),
    )
    .expect("serialized EWS request is UTF-8");

    for field in [
        "item:Subject",
        "calendar:Start",
        "calendar:End",
        "calendar:CalendarItemType",
        "calendar:IsRecurring",
        "calendar:Recurrence",
        "calendar:IsAllDayEvent",
        "calendar:IsMeeting",
        "calendar:IsOnlineMeeting",
        "calendar:RequiredAttendees",
        "calendar:OptionalAttendees",
        "calendar:Resources",
    ] {
        assert!(xml.contains(&format!("FieldURI=\"{field}\"")));
    }
    assert!(xml.contains(&format!(
        "PropertySetId=\"{OPERATION_MARKER_PROPERTY_SET_ID}\""
    )));
    assert!(xml.contains(&format!(
        "PropertyName=\"{OPERATION_MARKER_PROPERTY_NAME}\""
    )));
    assert!(xml.contains("PropertyType=\"String\""));
    assert!(xml.contains("Id=\"synthetic-calendar-event\""));
    assert!(!xml.contains("ChangeKey="));
    assert!(xml.contains("<t:BaseShape>IdOnly</t:BaseShape>"));
}

#[test]
fn parses_a_versioned_timed_standalone_calendar_item() {
    let snapshot = parse_calendar_item_snapshot(SUPPORTED_ITEM, &expected_id())
        .expect("synthetic standalone appointment is eligible");

    assert_eq!(snapshot.id, expected_id());
    assert_eq!(snapshot.change_key, "synthetic-change-key");
    assert_eq!(snapshot.subject, "Synthetic calendar appointment");
    assert_eq!(snapshot.start.unix_timestamp(), 1_794_477_600);
    assert_eq!(snapshot.end.unix_timestamp(), 1_794_481_200);
    assert_eq!(
        snapshot.operation_marker,
        Some(
            CalendarEventCreationId::parse("00000000-0000-4000-8000-000000000401")
                .expect("synthetic operation ID is valid")
        )
    );
}

#[test]
fn omitted_or_empty_attendee_collections_and_marker_are_supported() {
    let without_marker = appointment_xml().replace(
        concat!(
            "              <t:ExtendedProperty>\n",
            "                <t:ExtendedFieldURI PropertySetId=\"4c2a2922-972e-54f9-bf4a-21e6b6d66d82\"\n",
            "                                    PropertyName=\"EinfachMailCalendarOperationId\"\n",
            "                                    PropertyType=\"String\"/>\n",
            "                <t:Value>00000000-0000-4000-8000-000000000401</t:Value>\n",
            "              </t:ExtendedProperty>\n"
        ),
        "",
    );
    assert!(
        parse_calendar_item_snapshot(without_marker.as_bytes(), &expected_id())
            .expect("marker is optional for existing appointments")
            .operation_marker
            .is_none()
    );

    let with_empty_collections = appointment_xml().replace(
        "            </t:CalendarItem>",
        concat!(
            "              <t:RequiredAttendees/>\n",
            "              <t:OptionalAttendees/>\n",
            "              <t:Resources/>\n",
            "            </t:CalendarItem>"
        ),
    );
    assert!(
        parse_calendar_item_snapshot(with_empty_collections.as_bytes(), &expected_id()).is_ok()
    );
}

#[test]
fn rejects_meetings_recurring_all_day_online_and_non_single_items() {
    let fixture = appointment_xml();
    let cases = [
        (
            "meeting",
            replace_once(
                &fixture,
                "<t:IsMeeting>false</t:IsMeeting>",
                "<t:IsMeeting>true</t:IsMeeting>",
            ),
        ),
        (
            "recurring",
            replace_once(
                &fixture,
                "<t:IsRecurring>false</t:IsRecurring>",
                "<t:IsRecurring>true</t:IsRecurring>",
            ),
        ),
        (
            "all-day",
            replace_once(
                &fixture,
                "<t:IsAllDayEvent>false</t:IsAllDayEvent>",
                "<t:IsAllDayEvent>true</t:IsAllDayEvent>",
            ),
        ),
        (
            "online meeting",
            replace_once(
                &fixture,
                "<t:IsOnlineMeeting>false</t:IsOnlineMeeting>",
                "<t:IsOnlineMeeting>true</t:IsOnlineMeeting>",
            ),
        ),
        (
            "recurrence item type",
            replace_once(
                &fixture,
                "<t:CalendarItemType>Single</t:CalendarItemType>",
                "<t:CalendarItemType>Occurrence</t:CalendarItemType>",
            ),
        ),
    ];

    for (name, document) in cases {
        assert_eq!(
            parse_calendar_item_snapshot(document.as_bytes(), &expected_id()),
            Err(CalendarItemSnapshotError::Ineligible),
            "{name} items are ineligible"
        );
    }
    assert_eq!(
        parse_calendar_item_snapshot(CONTRADICTORY_RECURRENCE_ITEM, &expected_id()),
        Err(CalendarItemSnapshotError::Ineligible),
        "recurrence data is ineligible even when scalar fields contradict it"
    );
    assert_eq!(
        parse_calendar_item_snapshot(NIL_RECURRENCE_ITEM, &expected_id()),
        Err(CalendarItemSnapshotError::Ineligible),
        "a nil recurrence element cannot hide recurrence content"
    );
}

#[test]
fn rejects_required_optional_and_resource_attendees() {
    let fixture = appointment_xml();
    let cases = [
        (
            "required attendee",
            "<t:RequiredAttendees><t:Attendee/></t:RequiredAttendees>",
        ),
        (
            "optional attendee",
            "<t:OptionalAttendees><t:Attendee/></t:OptionalAttendees>",
        ),
        (
            "resource attendee",
            "<t:Resources><t:Attendee/></t:Resources>",
        ),
        (
            "malformed attendee collection",
            "<t:RequiredAttendees><t:UnsupportedEntry/></t:RequiredAttendees>",
        ),
    ];

    for (name, collection) in cases {
        let document = fixture.replace(
            "            </t:CalendarItem>",
            &format!("{collection}\n            </t:CalendarItem>"),
        );
        assert_eq!(
            parse_calendar_item_snapshot(document.as_bytes(), &expected_id()),
            Err(CalendarItemSnapshotError::Ineligible),
            "{name} makes the appointment ineligible"
        );
    }
}

#[test]
fn missing_required_values_and_mismatched_identity_are_ineligible() {
    let fixture = appointment_xml();
    let missing_values = [
        "              <t:Subject>Synthetic calendar appointment</t:Subject>\n",
        "              <t:Start>2026-11-12T10:00:00Z</t:Start>\n",
        "              <t:End>2026-11-12T11:00:00Z</t:End>\n",
        "              <t:CalendarItemType>Single</t:CalendarItemType>\n",
        "              <t:IsRecurring>false</t:IsRecurring>\n",
        "              <t:IsAllDayEvent>false</t:IsAllDayEvent>\n",
        "              <t:IsMeeting>false</t:IsMeeting>\n",
        "              <t:IsOnlineMeeting>false</t:IsOnlineMeeting>\n",
    ];

    for value in missing_values {
        assert_ineligible(&fixture.replace(value, ""));
    }
    assert_ineligible(&replace_once(
        &fixture,
        "Id=\"synthetic-calendar-event\"",
        "Id=\"\"",
    ));
    assert_ineligible(&replace_once(
        &fixture,
        "ChangeKey=\"synthetic-change-key\"",
        "ChangeKey=\"\"",
    ));
    assert_ineligible(&replace_once(
        &fixture,
        "<t:IsRecurring>false</t:IsRecurring>",
        "<t:IsRecurring>maybe</t:IsRecurring>",
    ));
    assert_ineligible(&replace_once(
        &fixture,
        "2026-11-12T10:00:00Z",
        "2026-11-12T11:00:00+01:00",
    ));
    assert_ineligible(&replace_once(
        &fixture,
        "2026-11-12T11:00:00Z",
        "2026-11-12T09:00:00Z",
    ));
    let another_id = EventId::try_from("another-synthetic-event".to_owned())
        .expect("synthetic event ID is valid");
    assert_eq!(
        parse_calendar_item_snapshot(SUPPORTED_ITEM, &another_id),
        Err(CalendarItemSnapshotError::Ineligible)
    );
}

#[test]
fn parses_the_marker_only_when_its_metadata_and_value_match() {
    let wrong_property_set = replace_once(
        &appointment_xml(),
        OPERATION_MARKER_PROPERTY_SET_ID,
        "00020329-0000-0000-c000-000000000046",
    );
    assert!(
        parse_calendar_item_snapshot(wrong_property_set.as_bytes(), &expected_id())
            .expect("unrelated extended properties do not disqualify the item")
            .operation_marker
            .is_none()
    );

    let invalid_marker = replace_once(
        &appointment_xml(),
        "00000000-0000-4000-8000-000000000401",
        "not-a-uuid",
    );
    assert_ineligible(&invalid_marker);
}

#[test]
fn rejects_wrong_item_kind_and_malformed_or_incomplete_soap() {
    assert_eq!(
        parse_calendar_item_snapshot(WRONG_KIND_ITEM, &expected_id()),
        Err(CalendarItemSnapshotError::Ineligible)
    );
    assert_eq!(
        parse_calendar_item_snapshot(MALFORMED_ITEM, &expected_id()),
        Err(CalendarItemSnapshotError::InvalidResponse)
    );
    assert_eq!(
        parse_calendar_item_snapshot(INCOMPLETE_ITEM, &expected_id()),
        Err(CalendarItemSnapshotError::InvalidResponse)
    );
}

#[test]
fn accepts_only_a_matching_error_class_and_response_code() {
    assert_eq!(
        parse_calendar_item_snapshot(REJECTED_ITEM, &expected_id()),
        Err(CalendarItemSnapshotError::Rejected(
            ResponseCode::ErrorItemNotFound
        ))
    );
    let contradictory = String::from_utf8(REJECTED_ITEM.to_vec())
        .expect("synthetic SOAP fixture is UTF-8")
        .replace("ResponseClass=\"Error\"", "ResponseClass=\"Success\"");
    assert_eq!(
        parse_calendar_item_snapshot(contradictory.as_bytes(), &expected_id()),
        Err(CalendarItemSnapshotError::InvalidResponse)
    );
}

#[test]
fn rejects_a_calendar_field_in_the_wrong_xml_namespace() {
    let document = replace_once(
        &appointment_xml(),
        "xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"",
        "xmlns:t=\"urn:synthetic:wrong-types\"",
    );
    assert_eq!(
        parse_calendar_item_snapshot(document.as_bytes(), &expected_id()),
        Err(CalendarItemSnapshotError::InvalidResponse)
    );
}

#[test]
fn rejects_namespaced_response_and_item_identity_attributes() {
    for attribute in ["ResponseClass", "Id", "ChangeKey"] {
        let document = String::from_utf8(NAMESPACED_ATTRIBUTES_ITEM.to_vec())
            .expect("synthetic SOAP fixture is UTF-8");
        assert!(
            document.contains(&format!("x:{attribute}=\"")),
            "fixture contains namespaced {attribute}"
        );
    }
    assert_eq!(
        parse_calendar_item_snapshot(NAMESPACED_ATTRIBUTES_ITEM, &expected_id()),
        Err(CalendarItemSnapshotError::InvalidResponse)
    );

    let fixture = appointment_xml().replace(
        "xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"",
        "xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\" xmlns:x=\"urn:synthetic\"",
    );
    let cases = [
        replace_once(
            &fixture,
            "ResponseClass=\"Success\"",
            "x:ResponseClass=\"Success\"",
        ),
        replace_once(
            &fixture,
            "Id=\"synthetic-calendar-event\"",
            "x:Id=\"synthetic-calendar-event\"",
        ),
        replace_once(
            &fixture,
            "ChangeKey=\"synthetic-change-key\"",
            "x:ChangeKey=\"synthetic-change-key\"",
        ),
    ];
    for document in cases {
        assert_eq!(
            parse_calendar_item_snapshot(document.as_bytes(), &expected_id()),
            Err(CalendarItemSnapshotError::InvalidResponse)
        );
    }
}
