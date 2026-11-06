//! Get the raw content of a Microsoft Graph attachment
//! (`GET /me/messages/{id}/attachments/{aid}/$value`).
//!
//! Like the message `$value` endpoint, this returns the decoded
//! attachment bytes rather than JSON, so it runs the HTTP send
//! directly and yields the body bytes.
//!
//! <https://learn.microsoft.com/en-us/graph/api/attachment-get>

use alloc::{format, vec::Vec};

use io_http::{
    coroutine::{HttpCoroutine, HttpCoroutineState},
    rfc6750::bearer::HttpAuthBearer,
    rfc9110::{
        request::HttpRequest,
        send::{HttpSendOutput, HttpSendYield},
    },
    rfc9112::send::Http11Send,
};
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    v1::send::{MSGRAPH_API_BASE, MsgraphSendError, MsgraphSendOutput, parse_api_error, user_path},
};

/// Gets the raw content of a Microsoft Graph attachment.
pub struct MsgraphAttachmentGetRaw {
    send: Http11Send,
}

impl MsgraphAttachmentGetRaw {
    /// Gets the raw content of the attachment `attachment_id` of the
    /// message `message_id`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph attachment raw retrieval");
        trace!("message_id: {message_id:?}");
        trace!("attachment_id: {attachment_id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!(
            "{user}/messages/{message_id}/attachments/{attachment_id}/$value"
        ))?;

        let request = HttpRequest::get(url.clone())
            .header("Authorization", auth.to_authorization())
            .body(Vec::new());

        Ok(Self {
            send: Http11Send::new(request),
        })
    }
}

impl MsgraphCoroutine for MsgraphAttachmentGetRaw {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<Vec<u8>>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        match self.send.resume(arg) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                MsgraphCoroutineState::Yielded(MsgraphYield::WantsRead)
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                MsgraphCoroutineState::Yielded(MsgraphYield::WantsWrite(bytes))
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { .. }) => {
                MsgraphCoroutineState::Complete(Err(MsgraphSendError::UnexpectedRedirect))
            }
            HttpCoroutineState::Complete(Err(err)) => {
                MsgraphCoroutineState::Complete(Err(err.into()))
            }
            HttpCoroutineState::Complete(Ok(HttpSendOutput {
                response,
                keep_alive,
                ..
            })) => {
                if response.status.is_success() {
                    debug!("attachment raw retrieved");
                    MsgraphCoroutineState::Complete(Ok(MsgraphSendOutput {
                        response: response.body,
                        keep_alive,
                    }))
                } else {
                    let (status, code, message) = parse_api_error(*response.status, &response.body);
                    MsgraphCoroutineState::Complete(Err(MsgraphSendError::Api {
                        status,
                        code,
                        message,
                    }))
                }
            }
        }
    }
}
