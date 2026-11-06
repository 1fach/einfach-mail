//! Combine several Microsoft Graph requests into one HTTP call (`POST
//! /$batch`).
//!
//! <https://learn.microsoft.com/en-us/graph/json-batching>

use alloc::{collections::BTreeMap, format, string::String, vec::Vec};

use base64_codec::codec::Base64Codec;
use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{
    Deserialize, Serialize,
    de::{DeserializeOwned, Error as _},
};
use serde_json::Value;
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::send::{
        MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, parse_api_error,
    },
};

/// Maximum number of requests Graph accepts in one batch.
pub const MSGRAPH_BATCH_MAX_REQUESTS: usize = 20;

/// One request of a batch.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphBatchRequest {
    /// The id correlating the request with its response, unique within
    /// the batch.
    pub id: String,
    /// The HTTP method.
    pub method: String,
    /// The URL relative to the API version, such as
    /// `/me/mailFolders/inbox`.
    pub url: String,
    /// The request headers, `Content-Type` being required with a body.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// The JSON request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// The ids of the requests this one runs after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

/// One response of a batch.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphBatchResponse {
    /// The id of the request this response answers.
    pub id: String,
    /// The HTTP status of the response.
    pub status: u16,
    /// The response headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// The JSON response body, or the error envelope on failure.
    #[serde(default)]
    pub body: Option<Value>,
}

impl MsgraphBatchResponse {
    /// Parses the body of a 2xx response into `T`, or turns any other
    /// status into [`MsgraphSendError::Api`].
    pub fn parse<T: DeserializeOwned>(self) -> Result<T, MsgraphSendError> {
        let body = self.body.unwrap_or(Value::Null);

        if (200..300).contains(&self.status) {
            return serde_json::from_value(body).map_err(MsgraphSendError::ParseResponse);
        }

        let body = serde_json::to_vec(&body).unwrap_or_default();
        let (status, code, message) = parse_api_error(self.status, &body);
        Err(MsgraphSendError::Api {
            status,
            code,
            message,
        })
    }

    /// Decodes the body of a 2xx response answered as a JSON string
    /// holding base64, or turns any other status into
    /// [`MsgraphSendError::Api`]. A missing body reads as empty.
    ///
    /// Line breaks, blanks and missing padding are accepted, any other byte
    /// outside the alphabet fails with [`MsgraphSendError::ParseResponse`].
    ///
    /// Graph encodes that way an inner body that is not JSON, such as
    /// the MIME message of a `$value` request. The documentation only
    /// shows JSON bodies, the encoding is community-observed:
    /// <https://learn.microsoft.com/en-us/graph/json-batching>.
    pub fn bytes(self) -> Result<Vec<u8>, MsgraphSendError> {
        let Some(encoded) = self.parse::<Option<String>>()? else {
            return Ok(Vec::new());
        };

        let mut decoded = Vec::new();

        if !Base64Codec::default().decode_vec(encoded.as_bytes(), &mut decoded) {
            let err = serde_json::Error::custom("Graph batch body is not base64");
            return Err(MsgraphSendError::ParseResponse(err));
        }

        Ok(decoded)
    }

    /// Returns the delay of the `Retry-After` header in seconds, `None`
    /// when absent or given as an HTTP date (RFC 9110 §10.2.3).
    ///
    /// Graph sends it on a throttled (429) inner response.
    pub fn retry_after(&self) -> Option<u64> {
        self.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
            .and_then(|(_, value)| value.trim().parse().ok())
    }
}

/// The batch response body, in no particular order.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphBatchResponses {
    /// The responses, one per request.
    #[serde(default)]
    pub responses: Vec<MsgraphBatchResponse>,
}

#[derive(Serialize)]
struct Body<'a> {
    requests: &'a [MsgraphBatchRequest],
}

/// Sends a Microsoft Graph batch.
pub struct MsgraphBatch {
    send: MsgraphSend<MsgraphBatchResponses>,
}

impl MsgraphBatch {
    /// Sends `requests`, between 1 and [`MSGRAPH_BATCH_MAX_REQUESTS`]
    /// with unique ids.
    pub fn new(
        auth: &HttpAuthBearer,
        requests: &[MsgraphBatchRequest],
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph batch");
        trace!("requests: {requests:?}");

        if requests.is_empty() || requests.len() > MSGRAPH_BATCH_MAX_REQUESTS {
            let err = MsgraphSendError::InvalidRequest(format!(
                "Batch must hold between 1 and {MSGRAPH_BATCH_MAX_REQUESTS} requests"
            ));
            return Err(err);
        }

        for (i, request) in requests.iter().enumerate() {
            if requests[..i].iter().any(|prev| prev.id == request.id) {
                let err = MsgraphSendError::InvalidRequest(format!(
                    "Batch request id `{}` is not unique",
                    request.id
                ));
                return Err(err);
            }
        }

        let url = Url::parse(MSGRAPH_API_BASE)?.join("$batch")?;
        let send = MsgraphSend::post_json(auth, url, &Body { requests })?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphBatch {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphBatchResponses>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("batch sent");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
