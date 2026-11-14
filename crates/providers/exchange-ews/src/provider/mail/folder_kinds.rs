use einfach_mail_domain::{FolderId, MailFolderKind, MailFolderKinds};
use ews::{
    BaseFolderId, BaseShape, Folder, FolderShape, ResponseClass,
    get_folder::{GetFolder, GetFolderResponseMessage},
    response::ResponseCode,
};

use super::map_response_code;
use crate::error::EwsError;

/// Pairs each requested distinguished folder ID with its kind, in request order.
///
/// `archive` is missing from Microsoft's `DistinguishedFolderId` list. Thunderbird's EWS client
/// requests it because it returns the mailbox's archive folder.
const DISTINGUISHED_FOLDER_KINDS: [(&str, MailFolderKind); 6] = [
    ("inbox", MailFolderKind::Inbox),
    ("deleteditems", MailFolderKind::DeletedItems),
    ("drafts", MailFolderKind::Drafts),
    ("sentitems", MailFolderKind::SentItems),
    ("junkemail", MailFolderKind::JunkEmail),
    ("archive", MailFolderKind::Archive),
];

pub(super) fn folder_kinds_request() -> GetFolder {
    GetFolder {
        folder_shape: FolderShape {
            base_shape: BaseShape::IdOnly,
        },
        folder_ids: DISTINGUISHED_FOLDER_KINDS
            .iter()
            .map(|(distinguished_id, _)| BaseFolderId::new_distinguished(*distinguished_id))
            .collect(),
    }
}

/// Maps the response messages of [`folder_kinds_request`] to folder kinds by request order.
///
/// A kind whose response is `ErrorFolderNotFound` or `ErrorSchemaValidation` is absent. A folder
/// that two IDs name keeps the kind of the earlier ID.
pub(super) fn map_folder_kinds(
    responses: Vec<ResponseClass<GetFolderResponseMessage>>,
) -> Result<MailFolderKinds, EwsError> {
    if responses.len() != DISTINGUISHED_FOLDER_KINDS.len() {
        return Err(EwsError::InvalidResponse);
    }
    let mut kinds = MailFolderKinds::new();
    for ((_, kind), response) in DISTINGUISHED_FOLDER_KINDS.into_iter().zip(responses) {
        let message = match response {
            ResponseClass::Success(message) | ResponseClass::Warning(message) => message,
            ResponseClass::Error(error)
                if matches!(
                    error.response_code,
                    ResponseCode::ErrorFolderNotFound | ResponseCode::ErrorSchemaValidation
                ) =>
            {
                continue;
            }
            ResponseClass::Error(error) => return Err(map_response_code(error.response_code)),
        };
        kinds.assign(kind, resolved_folder_id(message)?);
    }
    Ok(kinds)
}

fn resolved_folder_id(message: GetFolderResponseMessage) -> Result<FolderId, EwsError> {
    let mut folders = message.folders.inner.into_iter();
    let (Some(Folder::Folder { folder_id, .. }), None) = (folders.next(), folders.next()) else {
        return Err(EwsError::InvalidResponse);
    };
    let folder_id = folder_id.ok_or(EwsError::InvalidResponse)?;
    FolderId::try_from(folder_id.id).map_err(|_| EwsError::InvalidResponse)
}

#[cfg(test)]
mod tests;
