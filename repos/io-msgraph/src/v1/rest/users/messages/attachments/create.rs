//! Add a file attachment to a Microsoft Graph message
//! (`POST /me/messages/{id}/attachments`); returns the created
//! attachment.
//!
//! Only the `fileAttachment` subtype is created here: the raw content is
//! base64-encoded into `contentBytes`, as Graph requires.
//!
//! <https://learn.microsoft.com/en-us/graph/api/message-post-attachments>

use alloc::{format, string::String, vec::Vec};

use base64_codec::codec::Base64Codec;
use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::Serialize;
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::messages::attachments::MsgraphAttachment,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Body of the file attachment create request.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MsgraphAttachmentCreateRequest<'a> {
    #[serde(rename = "@odata.type")]
    odata_type: &'a str,
    name: &'a str,
    content_bytes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<&'a str>,
}

/// Adds a file attachment to a Microsoft Graph message.
pub struct MsgraphAttachmentCreate {
    send: MsgraphSend<MsgraphAttachment>,
}

impl MsgraphAttachmentCreate {
    /// Adds a `fileAttachment` named `name` carrying `content`, with
    /// an optional MIME `content_type`.
    ///
    /// The raw content bytes are base64-encoded into `contentBytes`,
    /// as Graph requires.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        message_id: &str,
        name: &str,
        content: &[u8],
        content_type: Option<&str>,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph attachment for creation");
        trace!("message_id: {message_id:?}");
        trace!("name: {name:?}");
        trace!("content_type: {content_type:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?
            .join(&format!("{user}/messages/{message_id}/attachments"))?;

        let mut content_bytes = Vec::new();
        Base64Codec::default().encode_vec(content, &mut content_bytes);
        // NOTE: the base64 alphabet is ASCII, so this never falls back.
        let content_bytes = String::from_utf8(content_bytes).unwrap_or_default();

        let body = MsgraphAttachmentCreateRequest {
            odata_type: "#microsoft.graph.fileAttachment",
            name,
            content_bytes,
            content_type,
        };
        let send = MsgraphSend::post_json(auth, url, &body)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphAttachmentCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphAttachment>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("attachment created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
