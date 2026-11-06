//! List a Microsoft Graph message's attachments
//! (`GET /me/messages/{id}/attachments`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/message-list-attachments>

use alloc::{format, string::String, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::messages::attachments::MsgraphAttachment,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of attachments (`value` plus the OData paging link).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphAttachmentsListResponse {
    /// The attachments of the page.
    #[serde(default)]
    pub value: Vec<MsgraphAttachment>,
    /// The URL of the next page, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// Lists the attachments of a Microsoft Graph message.
pub struct MsgraphAttachmentsList {
    send: MsgraphSend<MsgraphAttachmentsListResponse>,
}

impl MsgraphAttachmentsList {
    /// Lists the attachments of the message `message_id`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        message_id: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph attachments listing");
        trace!("message_id: {message_id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?
            .join(&format!("{user}/messages/{message_id}/attachments"))?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphAttachmentsList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphAttachmentsListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("attachments listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
