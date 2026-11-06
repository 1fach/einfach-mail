//! Create a Microsoft Graph message from raw MIME (`POST /me/messages`
//! or `POST /me/mailFolders/{folder}/messages`); the MIME is
//! base64-encoded and posted as `text/plain`, as Graph requires, and the
//! created draft message resource is returned.
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-post-messages>

use alloc::{format, vec::Vec};

use base64_codec::codec::Base64Codec;
use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::messages::MsgraphMessage,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Creates a Microsoft Graph draft message from raw RFC 5322 MIME.
pub struct MsgraphMessageCreateMime {
    send: MsgraphSend<MsgraphMessage>,
}

impl MsgraphMessageCreateMime {
    /// Creates the draft in the mailbox root, or in `folder` when given
    /// (a folder id or a well-known name such as `drafts`).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: Option<&str>,
        raw: &[u8],
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph message creation (mime)");
        trace!("folder: {folder:?}");
        trace!("raw len: {}", raw.len());

        let user = user_path(user_id);
        let path = match folder {
            Some(folder) => format!("{user}/mailFolders/{folder}/messages"),
            None => format!("{user}/messages"),
        };
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        let mut body = Vec::new();
        Base64Codec::default().encode_vec(raw, &mut body);

        let send = MsgraphSend::post_text(auth, url, body);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMessageCreateMime {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMessage>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("message created (mime)");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
