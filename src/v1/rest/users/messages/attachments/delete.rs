//! Delete a Microsoft Graph attachment
//! (`DELETE /me/messages/{id}/attachments/{aid}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/attachment-delete>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::send::{
        MSGRAPH_API_BASE, MsgraphNoResponse, MsgraphSend, MsgraphSendError, MsgraphSendOutput,
        user_path,
    },
};

/// Deletes an attachment of a Microsoft Graph message.
pub struct MsgraphAttachmentDelete {
    send: MsgraphSend<MsgraphNoResponse>,
}

impl MsgraphAttachmentDelete {
    /// Deletes the attachment `attachment_id` of the message
    /// `message_id`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph attachment deletion");
        trace!("message_id: {message_id:?}");
        trace!("attachment_id: {attachment_id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!(
            "{user}/messages/{message_id}/attachments/{attachment_id}"
        ))?;
        let send = MsgraphSend::delete(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphAttachmentDelete {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("attachment deleted");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
