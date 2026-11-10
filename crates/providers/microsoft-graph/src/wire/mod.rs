//! Microsoft Graph profile, mail, and calendar wire mapping.

use huskarl::token::AccessToken;
use serde::Deserialize;
use url::Url;

use crate::{
    error::GraphError,
    transport::{GraphAuthority, GraphReadOperation, GraphTransportHandle},
};
use einfach_mail_domain::{AccountId, AccountProfile};

mod calendar;
mod mail;
mod pagination;
mod shared;

#[cfg(test)]
mod tests;

pub(super) struct GraphApi {
    authority: GraphAuthority,
    transport: GraphTransportHandle,
}

impl GraphApi {
    pub(super) fn new(authority: GraphAuthority, transport: GraphTransportHandle) -> Self {
        Self {
            authority,
            transport,
        }
    }

    pub(super) async fn load_profile(
        &self,
        endpoint: &Url,
        token: std::sync::Arc<AccessToken>,
    ) -> Result<AccountProfile, GraphError> {
        let profile_url = self.authority.profile_url(endpoint)?;
        let response = self
            .transport
            .get(GraphReadOperation::Profile, profile_url, token)
            .await?;
        let profile = serde_json::from_slice::<GraphUserProfile>(&response)
            .map_err(|_| GraphError::InvalidResponse)?;
        profile.try_into()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphUserProfile {
    id: Option<String>,
    display_name: Option<String>,
    mail: Option<String>,
    user_principal_name: Option<String>,
}

impl TryFrom<GraphUserProfile> for AccountProfile {
    type Error = GraphError;

    fn try_from(profile: GraphUserProfile) -> Result<Self, Self::Error> {
        let id = profile
            .id
            .and_then(|id| AccountId::try_from(id).ok())
            .ok_or(GraphError::InvalidRecord)?;
        AccountProfile::try_new(
            id,
            profile.display_name,
            profile.mail,
            profile.user_principal_name,
        )
        .map_err(|_| GraphError::InvalidRecord)
    }
}
