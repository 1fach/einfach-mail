use ews::{BaseItemId, DeleteType, delete_item::DeleteItem};

use super::super::item::CalendarItemSnapshot;
use crate::{error::EwsError, transport::serialize_soap_request};
use ews::delete_item::SendMeetingCancellations;

pub(super) fn delete_item_request(snapshot: &CalendarItemSnapshot) -> Result<Vec<u8>, EwsError> {
    serialize_soap_request(DeleteItem {
        delete_type: DeleteType::MoveToDeletedItems,
        send_meeting_cancellations: Some(SendMeetingCancellations::SendToNone),
        affected_task_occurrences: None,
        suppress_read_receipts: None,
        item_ids: vec![BaseItemId::ItemId {
            id: snapshot.id.as_str().to_owned(),
            change_key: Some(snapshot.change_key.clone()),
        }],
    })
}
