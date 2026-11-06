//! Update a Microsoft Graph contact (`PATCH /me/contacts/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/contact-update>

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

/// Updates a Microsoft Graph contact.
pub struct MsgraphContactUpdate {
    send: MsgraphSend<MsgraphContact>,
}

impl MsgraphContactUpdate {
    /// Patches the contact `id` with the set and null fields of
    /// `contact`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        contact: &MsgraphContact,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact update");
        trace!("id: {id:?}");
        trace!("contact: {contact:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/contacts/{id}"))?;
        let send = MsgraphSend::patch_json(auth, url, contact)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactUpdate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContact>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact updated");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
