//! Get a Microsoft Graph contact (`GET /me/contacts/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/contact-get>

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

/// Gets a Microsoft Graph contact.
pub struct MsgraphContactGet {
    send: MsgraphSend<MsgraphContact>,
}

impl MsgraphContactGet {
    /// Gets the contact `id`, `$expand`ing the given navigation clause
    /// when one is passed.
    ///
    /// Graph omits extended properties from responses unless the
    /// request expands them (e.g. a filtered extended-property
    /// expansion).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        expand: Option<&str>,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact retrieval");
        trace!("id: {id:?}");
        trace!("expand: {expand:?}");

        let user = user_path(user_id);
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/contacts/{id}"))?;

        if let Some(expand) = expand {
            url.query_pairs_mut().append_pair("$expand", expand);
        }

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactGet {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContact>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact retrieved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
