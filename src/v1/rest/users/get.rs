//! Get a Microsoft Graph user (`GET /me`, `GET /users/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-get>

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::MsgraphUser,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Gets a Microsoft Graph user profile.
pub struct MsgraphUserGet {
    send: MsgraphSend<MsgraphUser>,
}

impl MsgraphUserGet {
    /// Gets the profile of `user_id` (`me`, a user id or a principal
    /// name).
    pub fn new(auth: &HttpAuthBearer, user_id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph user retrieval");
        trace!("user_id: {user_id:?}");

        let url = Url::parse(MSGRAPH_API_BASE)?.join(&user_path(user_id))?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphUserGet {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphUser>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("user retrieved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
