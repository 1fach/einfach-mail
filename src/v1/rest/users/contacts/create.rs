//! Create a Microsoft Graph contact (`POST /me/contacts` or
//! `POST /me/contactFolders/{id}/contacts`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-post-contacts>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::contacts::MsgraphContact,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Creates a Microsoft Graph contact.
pub struct MsgraphContactCreate {
    send: MsgraphSend<MsgraphContact>,
}

impl MsgraphContactCreate {
    /// Creates the contact in the default Contacts folder, or in
    /// `folder` when given (a contact folder id).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: Option<&str>,
        contact: &MsgraphContact,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact for creation");
        trace!("folder: {folder:?}");
        trace!("contact: {contact:?}");

        let user = user_path(user_id);
        let path = match folder {
            Some(folder) => format!("{user}/contactFolders/{folder}/contacts"),
            None => format!("{user}/contacts"),
        };
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        let send = MsgraphSend::post_json(auth, url, contact)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContact>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
