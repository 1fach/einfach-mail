use einfach_mail_domain::{FolderId, MailFolderKind, MailFolderKinds};
use ews::get_folder::GetFolder;

use crate::{
    error::EwsError,
    transport::{parse_soap_response, serialize_soap_request},
};

use super::{folder_kinds_request, map_folder_kinds};

const GET_FOLDER_KINDS_RESOLVED: &[u8] =
    include_bytes!("../../../fixtures/get-folder-kinds-resolved.xml");
const GET_FOLDER_KINDS_ARCHIVE_MISSING: &[u8] =
    include_bytes!("../../../fixtures/get-folder-kinds-archive-missing.xml");

fn kinds_from(document: &[u8]) -> Result<MailFolderKinds, EwsError> {
    let response =
        parse_soap_response::<GetFolder>(document).expect("synthetic GetFolder response parses");
    map_folder_kinds(response.messages)
}

fn kinds_from_text(document: &str) -> Result<MailFolderKinds, EwsError> {
    kinds_from(document.as_bytes())
}

fn assignments(kinds: &MailFolderKinds) -> Vec<(MailFolderKind, String)> {
    kinds
        .assignments()
        .map(|(kind, folder)| (kind, folder.as_str().to_owned()))
        .collect()
}

fn folder_id(id: &str) -> FolderId {
    FolderId::try_from(id.to_owned()).expect("synthetic folder ID is valid")
}

#[test]
fn request_names_six_distinguished_folders_once_with_ids_only() {
    let document = serialize_soap_request(folder_kinds_request())
        .expect("synthetic GetFolder request serializes");
    let xml = String::from_utf8(document).expect("SOAP request is UTF-8");

    assert!(xml.contains("<t:BaseShape>IdOnly</t:BaseShape>"), "{xml}");
    let requested: Vec<&str> = xml
        .split("<t:DistinguishedFolderId Id=\"")
        .skip(1)
        .filter_map(|element| element.split('"').next())
        .collect();
    assert_eq!(
        requested,
        [
            "inbox",
            "deleteditems",
            "drafts",
            "sentitems",
            "junkemail",
            "archive"
        ],
        "{xml}"
    );
}

#[test]
fn maps_each_resolved_folder_to_its_kind_by_request_order() {
    let kinds = kinds_from(GET_FOLDER_KINDS_RESOLVED).expect("every folder kind resolves");

    assert_eq!(
        assignments(&kinds),
        [
            (MailFolderKind::Inbox, "synthetic-inbox-id".to_owned()),
            (
                MailFolderKind::DeletedItems,
                "synthetic-deleted-items-id".to_owned()
            ),
            (MailFolderKind::Drafts, "synthetic-drafts-id".to_owned()),
            (
                MailFolderKind::SentItems,
                "synthetic-sent-items-id".to_owned()
            ),
            (
                MailFolderKind::JunkEmail,
                "synthetic-junk-email-id".to_owned()
            ),
            (MailFolderKind::Archive, "synthetic-archive-id".to_owned()),
        ]
    );
}

#[test]
fn a_folder_the_mailbox_lacks_is_absent_and_the_other_kinds_resolve() {
    let archive_missing = String::from_utf8_lossy(GET_FOLDER_KINDS_ARCHIVE_MISSING);
    for document in [
        archive_missing.to_string(),
        archive_missing.replace("ErrorFolderNotFound", "ErrorSchemaValidation"),
    ] {
        let kinds = kinds_from_text(&document).expect("a missing folder is not an error");

        assert_eq!(kinds.folder_of(MailFolderKind::Archive), None);
        assert_eq!(kinds.kind_of(&folder_id("synthetic-archive-id")), None);
        assert_eq!(
            kinds.folder_of(MailFolderKind::JunkEmail),
            Some(&folder_id("synthetic-junk-email-id"))
        );
        assert_eq!(assignments(&kinds).len(), 5);
    }
}

#[test]
fn any_other_error_response_fails_the_whole_resolution() {
    let archive_missing = String::from_utf8_lossy(GET_FOLDER_KINDS_ARCHIVE_MISSING);
    for (response_code, expected_error) in [
        ("ErrorInternalServerError", EwsError::InvalidResponse),
        ("ErrorAccessDenied", EwsError::AccessDenied),
    ] {
        let document = archive_missing.replace("ErrorFolderNotFound", response_code);

        assert_eq!(kinds_from_text(&document), Err(expected_error));
    }
}

#[test]
fn a_response_count_other_than_the_request_count_fails_the_resolution() {
    let mut responses = parse_soap_response::<GetFolder>(GET_FOLDER_KINDS_RESOLVED)
        .expect("synthetic GetFolder response parses")
        .messages;
    let extra = responses
        .last()
        .cloned()
        .expect("synthetic response has messages");

    let mut seven = responses.clone();
    seven.push(extra);
    assert_eq!(map_folder_kinds(seven), Err(EwsError::InvalidResponse));
    responses.pop();
    assert_eq!(map_folder_kinds(responses), Err(EwsError::InvalidResponse));
}

#[test]
fn a_folder_named_by_two_ids_keeps_the_earlier_kind() {
    let document = String::from_utf8_lossy(GET_FOLDER_KINDS_RESOLVED)
        .replace("synthetic-archive-id", "synthetic-inbox-id");

    let kinds = kinds_from_text(&document).expect("a repeated folder is not an error");

    assert_eq!(
        kinds.kind_of(&folder_id("synthetic-inbox-id")),
        Some(MailFolderKind::Inbox)
    );
    assert_eq!(kinds.folder_of(MailFolderKind::Archive), None);
    assert_eq!(assignments(&kinds).len(), 5);
}

#[test]
fn a_success_without_exactly_one_plain_folder_fails_the_resolution() {
    const ARCHIVE_FOLDER: &str = "<t:Folder>\n              \
        <t:FolderId Id=\"synthetic-archive-id\" ChangeKey=\"synthetic-change-key\"/>\n            \
        </t:Folder>";
    let resolved = String::from_utf8_lossy(GET_FOLDER_KINDS_RESOLVED);
    assert!(resolved.contains(ARCHIVE_FOLDER));
    for archive_folders in [
        "",
        "<t:Folder></t:Folder>",
        "<t:SearchFolder><t:FolderId Id=\"synthetic-archive-id\"/></t:SearchFolder>",
        "<t:Folder><t:FolderId Id=\"synthetic-archive-id\"/></t:Folder>\
         <t:Folder><t:FolderId Id=\"synthetic-second-id\"/></t:Folder>",
    ] {
        let document = resolved.replace(ARCHIVE_FOLDER, archive_folders);

        assert_eq!(kinds_from_text(&document), Err(EwsError::InvalidResponse));
    }
}
