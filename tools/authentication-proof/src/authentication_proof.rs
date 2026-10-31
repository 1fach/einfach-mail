use std::{
    collections::HashSet,
    error::Error,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use http::{
    HeaderMap, Method, Request, StatusCode,
    header::{CONTENT_TYPE, HeaderValue, LOCATION},
};
use huskarl::{
    core::{
        Error as HuskarlError, RetryAdvice,
        client_auth::NoAuth,
        crypto::{
            KeyMatchStrength,
            verifier::{
                CreateVerifierError, JwsVerifier, JwsVerifierPlatform, KeyMatch, VerifyError,
            },
        },
        http::{HttpClient, HttpResponse, Idempotency},
        jwk::{KeyOperation, KeyUse, PublicJwk, PublicKey},
        jwt::validator::ValidatedJwt,
        platform::MaybeSendBoxFuture,
    },
    grant::authorization_code::{AuthorizationCodeGrant, LoopbackError, PendingState, StartInput},
    token::id_token::IdTokenClaims,
};
use huskarl_reqwest::ReqwestClient;
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode,
    errors::{Error as JsonwebtokenError, ErrorKind},
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};
use url::Url;

type ProofError = Box<dyn Error + Send + Sync>;
type ProofResult<T> = Result<T, ProofError>;

const CLIENT_ID: &str = "synthetic-desktop-client";
const ISSUER: &str = "https://issuer.example.test";
const CALLBACK_PATH: &str = "/callback";
const TEST_KEY_ID: &str = "synthetic-signing-key";
const SYNTHETIC_CODE: &str = "synthetic-authorization-code";
const RS256_ALGORITHMS: &[&str] = &["RS256"];
const JWKS_RESPONSE: &str = r#"{"keys":[{"kty":"RSA","n":"pyafoLADweUKq014AefApo87uNG-coNpRvHJdGFiH2TDvRAj-DW0M1w7iVcm9jyTMMWGtKuk7EjFYScOAriIHnVXVkm4axMOQCnj300YMZaMySDQGC9FtjMHCwpD0daJ6MCz6QOYHmrwyJfj3-bLx9ybp8x5DCmSd9Xg7mWpIYnOk8oAyLMLJAWVUCJhVT6eYB4i55UtnKyzAmBCWHRNN0wxggdcxpmPbb2k1sFEkmya9fI4XJaOslZlseCWa-HIocGNQ-Zk6zakYKvsTgIF8miFzF_Y1w-fyZERcuNNAUg4KXM6BiWhpPPHBWK6XxTgSMX_7I4YR_AO62RyHE0UhQ","e":"AQAB","use":"sig","alg":"RS256","kid":"synthetic-signing-key"}]}"#;

// This synthetic key signs runtime-generated tokens and contains no real identity data.
const SYNTHETIC_PRIVATE_KEY_PEM: &str = r"-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCnJp+gsAPB5Qqr
TXgB58Cmjzu40b5yg2lG8cl0YWIfZMO9ECP4NbQzXDuJVyb2PJMwxYa0q6TsSMVh
Jw4CuIgedVdWSbhrEw5AKePfTRgxlozJINAYL0W2MwcLCkPR1onowLPpA5geavDI
l+Pf5svH3JunzHkMKZJ31eDuZakhic6TygDIswskBZVQImFVPp5gHiLnlS2crLMC
YEJYdE03TDGCB1zGmY9tvaTWwUSSbJr18jhclo6yVmWx4JZr4cihwY1D5mTrNqRg
q+xOAgXyaIXMX9jXD5/JkRFy400BSDgpczoGJaGk88cFYrpfFOBIxf/sjhhH8A7r
ZHIcTRSFAgMBAAECggEADw0506D1/ahP4SQdOeKyJO2B/b8ZbLtrqAFO2EWKrfQH
ckd5TbkcTIGyNMhd0E+dG4e1/R/1vndBGuIG73TJgSCk6DbEPhQdtOZwyCNrmc+K
RvRG+N6M4v7OVA6tE8DDc2ui58k0kgykORQAZ6GjWRCDDcWqYs99in/+fMk3xTa7
wZNDzqlhmclq87lOSN22rWNnlLQfe8EIBZ/456GEeh+vyA4RC8fNzouczKiDHRe+
BzEwusoiEgqOqZUI66OMg0BG0Md0Lpcc630jEXCMGm1Kk53Aw63MG5+ET5KgSIlX
9ygl6VC1NiCgfs9uvGZQ344WUagnXpTWTCTUoN1pkwKBgQDet+z0hnG5A8Q3Pt5g
1yZcufNRrGujQM857awL+u4DeQX5IirmjR9muBrwDmdlPlozlLqjqbHKWbLwjwqo
lDREbxkljXV3al3vtb7Ae8Z7kWQqEf4w/mWUzfE07pIzNOhtxJnZ4yJvHa6AcVMg
pRMPfNtTaJsCS2lvoMhGFVrAgwKBgQDAIPcJcT93QD8UZ3K4dGDDIWGVmiy4f166
1hvMtjdE1df0lahQ7OX+tCR7CJ/7f5ZKtY7GkEiFyVTCc+bu9npjTcHM70cA+n1T
7bSlaJ8zlECAu/NTrDhQHEiFYvuhC+APxz+/HcIr4HxyvZ73Jm4e4k1HhNyXw/6M
VfKzbPM4VwKBgHbbk0KACWrifrG5BHaL+3Ya6qaYrKzoJm9Uzq9qD+Gckbp+KIT3
pV1tPTsFhNp8JylF40D6i95gdmV5AQgsZ6Rp4fKvhpPfGRiXVqBkre0+D0WAarZ8
3XtHWMWV2EIppy0FcdHdLIA+R+VWFTXjwsNOL8JYnfjgg/lFxfvY46C/AoGAPf0k
14YTfQ1cjN/Hst3RroOhsZwZ27k0vkGAnK8Z6/q2/7IdqWaV2nUS5CNnQsTqn9Yu
Z8B7tba8wzSUCPu7hzi6U73MAfPP5flm9Itp+kFWAnMh0hhZ3vcRKwBBwbdMJqsZ
1TbrxC5c2MR3erSKiSpo1q92YP0arvq4IM4BN2cCgYAPTalC8MBcvUvZX8JtnQnS
oHJk35G3AISd2IJOkDykRpfB3x+tWm5Ex+DpUE3jHnGpixfqkAyx/2zEg7wmhSdH
pQaWkWG4eocoKMmcy7HzSlGFAaa75vzGi8et/oSr06iPnAqcbeZ2avJn8KHXM4bi
2n+ztyqkTQLBCTQIgsXGMQ==
-----END PRIVATE KEY-----";

#[derive(Debug, Default)]
struct Rs256Platform;

impl JwsVerifierPlatform for Rs256Platform {
    fn create_verifier_from_jwk(
        &self,
        jwk: PublicJwk,
    ) -> MaybeSendBoxFuture<'static, Result<Arc<dyn JwsVerifier>, CreateVerifierError>> {
        Box::pin(async move {
            let verifier = Rs256Verifier::from_public_jwk(jwk)?;
            Ok(Arc::new(verifier) as Arc<dyn JwsVerifier>)
        })
    }

    fn supported_signature_algorithms(&self) -> &[&str] {
        RS256_ALGORITHMS
    }
}

#[derive(Debug)]
struct Rs256Verifier {
    decoding_key: DecodingKey,
    key_id: Option<String>,
}

impl Rs256Verifier {
    fn from_public_jwk(jwk: PublicJwk) -> Result<Self, CreateVerifierError> {
        if !jwk_is_acceptable(&jwk) {
            return Err(unsupported_jwk());
        }

        let PublicKey::Rsa(rsa_key) = jwk.key else {
            return Err(unsupported_jwk());
        };
        let decoding_key = DecodingKey::from_rsa_raw_components(&rsa_key.n, &rsa_key.e);
        Ok(Self {
            decoding_key,
            key_id: jwk.kid,
        })
    }
}

impl JwsVerifier for Rs256Verifier {
    fn key_match(&self, requested_key: &KeyMatch<'_>) -> Option<KeyMatchStrength> {
        requested_key.strength_for(RS256_ALGORITHMS, self.key_id.as_deref())
    }

    fn verify<'a>(
        &'a self,
        input: &'a [u8],
        signature: &'a [u8],
        requested_key: &'a KeyMatch<'a>,
    ) -> MaybeSendBoxFuture<'a, Result<(), VerifyError>> {
        if self.key_match(requested_key).is_none() {
            return Box::pin(async { Err(VerifyError::NoMatchingKey) });
        }

        let compact_jws = std::str::from_utf8(input)
            .map(str::to_owned)
            .map(|signing_input| format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature)));
        let decoding_key = self.decoding_key.clone();
        Box::pin(async move {
            let compact_jws = compact_jws.map_err(|source| VerifyError::Other {
                source: HuskarlError::new(RetryAdvice::No, source),
            })?;
            let mut validation = Validation::new(Algorithm::RS256);
            validation.required_spec_claims.clear();
            validation.validate_exp = false;
            validation.validate_nbf = false;
            validation.validate_aud = false;

            // Huskarl validates issuer, audience, time, and nonce after this signature check.
            decode::<Value>(&compact_jws, &decoding_key, &validation)
                .map(|_| ())
                .map_err(map_jsonwebtoken_error)
        })
    }
}

fn jwk_is_acceptable(jwk: &PublicJwk) -> bool {
    !jwk.has_private_parameters
        && matches!(&jwk.key, PublicKey::Rsa(rsa_key) if has_minimum_rsa_strength(&rsa_key.n))
        && jwk
            .algorithm
            .as_deref()
            .is_none_or(|algorithm| algorithm == "RS256")
        && jwk.key_use.is_none_or(|key_use| key_use == KeyUse::Sign)
        && jwk
            .key_operations
            .as_ref()
            .is_none_or(|operations| operations.contains(&KeyOperation::Verify))
}

fn has_minimum_rsa_strength(modulus: &[u8]) -> bool {
    let Some(first_nonzero_index) = modulus.iter().position(|byte| *byte != 0) else {
        return false;
    };
    let first_nonzero_byte = modulus[first_nonzero_index];
    let trailing_bits = (modulus.len() - first_nonzero_index - 1) * u8::BITS as usize;
    let leading_bits = (u8::BITS - first_nonzero_byte.leading_zeros()) as usize;
    trailing_bits + leading_bits >= 2048
}

fn unsupported_jwk() -> CreateVerifierError {
    CreateVerifierError::UnsupportedKey {
        source: HuskarlError::new(
            RetryAdvice::No,
            io::Error::other("JWKS key is not a public RS256 signing key"),
        ),
    }
}

fn map_jsonwebtoken_error(error: JsonwebtokenError) -> VerifyError {
    if matches!(error.kind(), ErrorKind::InvalidSignature) {
        return VerifyError::SignatureMismatch;
    }

    VerifyError::Other {
        source: HuskarlError::new(RetryAdvice::No, error),
    }
}

#[derive(Clone)]
struct FakeIdentityProvider {
    token_response: Arc<Mutex<Option<String>>>,
    token_requests: Arc<Mutex<Vec<String>>>,
}

impl FakeIdentityProvider {
    fn new() -> Self {
        Self {
            token_response: Arc::new(Mutex::new(None)),
            token_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn set_token_response(
        &self,
        issuer: &str,
        audience: &str,
        nonce: &str,
        authorized_party: &str,
    ) -> ProofResult<()> {
        let id_token = sign_synthetic_id_token(issuer, audience, nonce, authorized_party)?;
        self.store_token_response(&id_token)
    }

    fn set_tampered_token_response(
        &self,
        issuer: &str,
        audience: &str,
        nonce: &str,
        authorized_party: &str,
    ) -> ProofResult<()> {
        let id_token = sign_synthetic_id_token(issuer, audience, nonce, authorized_party)?;
        let tampered_id_token = tamper_jws_signature(&id_token)?;
        self.store_token_response(&tampered_id_token)
    }

    fn store_token_response(&self, id_token: &str) -> ProofResult<()> {
        let response = synthetic_token_response(id_token);
        *self
            .token_response
            .lock()
            .map_err(|_| io::Error::other("synthetic token response lock poisoned"))? =
            Some(response);
        Ok(())
    }

    fn token_request_count(&self) -> ProofResult<usize> {
        let requests = self
            .token_requests
            .lock()
            .map_err(|_| io::Error::other("synthetic token request lock poisoned"))?;
        Ok(requests.len())
    }

    fn last_token_request(&self) -> ProofResult<String> {
        self.token_requests
            .lock()
            .map_err(|_| io::Error::other("synthetic token request lock poisoned"))?
            .last()
            .cloned()
            .ok_or_else(|| io::Error::other("no synthetic token request was captured").into())
    }

    fn token_response_for(&self, request_body: &Bytes) -> Result<String, HuskarlError> {
        let request_form = std::str::from_utf8(request_body)
            .map(str::to_owned)
            .map_err(|source| HuskarlError::new(RetryAdvice::No, source))?;
        self.token_requests
            .lock()
            .map_err(|_| synthetic_transport_error())?
            .push(request_form);
        self.token_response
            .lock()
            .map_err(|_| synthetic_transport_error())?
            .clone()
            .ok_or_else(synthetic_transport_error)
    }
}

impl HttpClient for FakeIdentityProvider {
    fn execute(
        &self,
        request: Request<Bytes>,
        _idempotency: Idempotency,
    ) -> MaybeSendBoxFuture<'_, Result<HttpResponse, HuskarlError>> {
        let request_path = request.uri().path().to_owned();
        let request_method = request.method().clone();
        let request_body = request.into_body();

        Box::pin(async move {
            let response_body = match (request_path.as_str(), request_method) {
                ("/jwks", Method::GET) => JWKS_RESPONSE.to_owned(),
                ("/token", Method::POST) => self.token_response_for(&request_body)?,
                _ => return Err(synthetic_transport_error()),
            };
            Ok(json_response(response_body))
        })
    }
}

fn synthetic_transport_error() -> HuskarlError {
    HuskarlError::new(
        RetryAdvice::No,
        io::Error::other("unexpected synthetic identity-provider request"),
    )
}

fn json_response(body: String) -> HttpResponse {
    let headers =
        HeaderMap::from_iter([(CONTENT_TYPE, HeaderValue::from_static("application/json"))]);
    HttpResponse {
        status: StatusCode::OK,
        headers,
        body: Bytes::from(body),
    }
}

struct StartedLogin {
    pending_state: PendingState,
    authorization_url: Url,
    callback_state: String,
    nonce: String,
}

struct CompletedLogin {
    result: Result<huskarl::grant::authorization_code::CompleteOutput, LoopbackError>,
}

struct AuthenticationFixture {
    grant: AuthorizationCodeGrant,
    identity_provider: FakeIdentityProvider,
    listener: Arc<TcpListener>,
    callback_uri: String,
}

impl AuthenticationFixture {
    async fn new() -> ProofResult<Self> {
        let listener = Arc::new(TcpListener::bind("127.0.0.1:0").await?);
        let callback_uri = format!(
            "http://127.0.0.1:{}/callback",
            listener.local_addr()?.port()
        );
        let identity_provider = FakeIdentityProvider::new();
        let grant = build_grant(identity_provider.clone(), &callback_uri).await?;
        Ok(Self {
            grant,
            identity_provider,
            listener,
            callback_uri,
        })
    }

    async fn start_login(&self) -> ProofResult<StartedLogin> {
        let start_output = self
            .grant
            .start(StartInput::scope(vec!["openid".into(), "profile".into()]))
            .await?;
        let authorization_url = Url::parse(&start_output.authorization_url.to_string())?;
        let callback_state = query_field(&authorization_url, "state")?;
        let nonce = query_field(&authorization_url, "nonce")?;
        Ok(StartedLogin {
            pending_state: start_output.pending_state,
            authorization_url,
            callback_state,
            nonce,
        })
    }

    async fn complete_login(&self, started: StartedLogin) -> ProofResult<CompletedLogin> {
        let callback_address = self.listener.local_addr()?;
        let callback_task = self.spawn_callback(started.pending_state);
        if let Err(error) = request_callback(callback_address, &started.callback_state).await {
            callback_task.abort();
            if let Err(join_error) = callback_task.await
                && !join_error.is_cancelled()
            {
                return Err(Box::new(join_error));
            }
            return Err(error);
        }
        Ok(CompletedLogin {
            result: callback_task.await?,
        })
    }

    fn spawn_callback(
        &self,
        pending_state: PendingState,
    ) -> tokio::task::JoinHandle<
        Result<huskarl::grant::authorization_code::CompleteOutput, LoopbackError>,
    > {
        let login_grant = self.grant.clone();
        let listener = Arc::clone(&self.listener);
        tokio::spawn(async move {
            login_grant
                .complete_on_loopback(&listener, &pending_state, None)
                .await
        })
    }
}

async fn request_callback(address: SocketAddr, state: &str) -> ProofResult<()> {
    let callback_path = format!("{CALLBACK_PATH}?code={SYNTHETIC_CODE}&state={state}");
    let callback_response = send_loopback_request(address, &callback_path).await?;
    let result_path = callback_result_path(&callback_response)
        .ok_or_else(|| io::Error::other("synthetic callback did not redirect to a result page"))?;
    send_loopback_request(address, result_path).await?;
    Ok(())
}

async fn build_grant(
    identity_provider: FakeIdentityProvider,
    callback_uri: &str,
) -> ProofResult<AuthorizationCodeGrant> {
    let grant = AuthorizationCodeGrant::builder()
        .client_id(CLIENT_ID)
        .client_auth(NoAuth)
        .http_client(identity_provider)
        .issuer(ISSUER)
        .token_endpoint(format!("{ISSUER}/token").parse()?)
        .authorization_endpoint(format!("{ISSUER}/authorize").parse()?)
        .maybe_jwks_uri(Some(format!("{ISSUER}/jwks").parse()?))
        .redirect_uri(callback_uri)
        .oidc(true)
        .allowed_id_token_signed_response_algs(HashSet::from(["RS256".to_owned()]))
        .jws_verifier_platform(Arc::new(Rs256Platform))
        .build()
        .await?;
    Ok(grant)
}

fn sign_synthetic_id_token(
    issuer: &str,
    audience: &str,
    nonce: &str,
    authorized_party: &str,
) -> ProofResult<String> {
    let current_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KEY_ID.to_owned());
    let claims = json!({
        "iss": issuer,
        "sub": "synthetic-subject",
        "aud": audience,
        "iat": current_time,
        "exp": current_time + 3600,
        "nonce": nonce,
        "azp": authorized_party,
    });
    let signing_key = EncodingKey::from_rsa_pem(SYNTHETIC_PRIVATE_KEY_PEM.as_bytes())?;
    Ok(encode(&header, &claims, &signing_key)?)
}

fn tamper_jws_signature(compact_jws: &str) -> ProofResult<String> {
    let (signing_input, encoded_signature) = compact_jws
        .rsplit_once('.')
        .ok_or_else(|| io::Error::other("synthetic ID token has no signature"))?;
    let mut signature = URL_SAFE_NO_PAD.decode(encoded_signature)?;
    let final_byte = signature
        .last_mut()
        .ok_or_else(|| io::Error::other("synthetic ID token signature is empty"))?;
    *final_byte ^= 1;
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn synthetic_token_response(id_token: &str) -> String {
    json!({
        "access_token": "runtime-only-synthetic-access-token",
        "token_type": "Bearer",
        "expires_in": 3600,
        "scope": "openid profile",
        "id_token": id_token,
    })
    .to_string()
}

fn query_field(url: &Url, name: &str) -> ProofResult<String> {
    url.query_pairs()
        .find_map(|(field, field_value)| (field == name).then(|| field_value.into_owned()))
        .ok_or_else(|| missing_field_error(name).into())
}

fn form_field(encoded_form: &str, name: &str) -> ProofResult<String> {
    url::form_urlencoded::parse(encoded_form.as_bytes())
        .find_map(|(field, field_value)| (field == name).then(|| field_value.into_owned()))
        .ok_or_else(|| missing_field_error(name).into())
}

fn missing_field_error(name: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("missing {name} parameter"))
}

fn callback_result_path(response: &str) -> Option<&'static str> {
    if response.contains("Location: /success") {
        Some("/success")
    } else if response.contains("Location: /failure") {
        Some("/failure")
    } else {
        None
    }
}

async fn send_loopback_request(address: SocketAddr, path: &str) -> ProofResult<String> {
    let mut stream = TcpStream::connect(address).await?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    Ok(String::from_utf8(response)?)
}

fn validate_authorized_party(
    id_token: &ValidatedJwt<IdTokenClaims>,
    expected_client_id: &str,
) -> ProofResult<()> {
    let authorized_party_mismatch = id_token
        .claims
        .azp
        .as_deref()
        .is_some_and(|authorized_party| authorized_party != expected_client_id);
    let missing_for_multiple_audiences = id_token.aud.len() > 1 && id_token.claims.azp.is_none();
    if authorized_party_mismatch || missing_for_multiple_audiences {
        return Err(io::Error::other("ID token has an invalid azp claim").into());
    }
    Ok(())
}

fn assert_pkce_round_trip(authorization_url: &Url, token_request: &str) -> ProofResult<()> {
    let authorization_challenge = query_field(authorization_url, "code_challenge")?;
    let verifier = form_field(token_request, "code_verifier")?;
    let derived_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    assert_eq!(authorization_challenge, derived_challenge);
    assert_eq!(form_field(token_request, "code")?, SYNTHETIC_CODE);
    Ok(())
}

#[tokio::test]
async fn delegated_flow_validates_pkce_state_oidc_claims_and_authorized_party() -> ProofResult<()> {
    let fixture = AuthenticationFixture::new().await?;
    assert_forged_state_is_rejected(&fixture).await?;
    assert_tampered_signature_is_rejected(&fixture).await?;
    assert_issuer_mismatch_is_rejected(&fixture).await?;
    assert_audience_mismatch_is_rejected(&fixture).await?;
    assert_nonce_mismatch_is_rejected(&fixture).await?;
    assert_authorized_party_mismatch_is_rejected(&fixture).await?;
    assert_matching_flow_succeeds(&fixture).await?;
    Ok(())
}

async fn assert_tampered_signature_is_rejected(fixture: &AuthenticationFixture) -> ProofResult<()> {
    let tampered_signature_login = fixture.start_login().await?;
    fixture.identity_provider.set_tampered_token_response(
        ISSUER,
        CLIENT_ID,
        &tampered_signature_login.nonce,
        CLIENT_ID,
    )?;
    let token_requests_before = fixture.identity_provider.token_request_count()?;
    let tampered_signature_result = fixture.complete_login(tampered_signature_login).await?;
    assert!(tampered_signature_result.result.is_err());
    assert_eq!(
        fixture.identity_provider.token_request_count()?,
        token_requests_before + 1
    );
    Ok(())
}

async fn assert_forged_state_is_rejected(fixture: &AuthenticationFixture) -> ProofResult<()> {
    let forged_state_login = fixture.start_login().await?;
    let forged_callback = format!(
        "{}?code={SYNTHETIC_CODE}&state=forged-state",
        fixture.callback_uri
    );
    let forged_result = fixture
        .grant
        .complete(&forged_state_login.pending_state, forged_callback.parse()?)
        .await;
    assert!(forged_result.is_err());
    assert_eq!(fixture.identity_provider.token_request_count()?, 0);
    Ok(())
}

async fn assert_nonce_mismatch_is_rejected(fixture: &AuthenticationFixture) -> ProofResult<()> {
    let wrong_nonce_login = fixture.start_login().await?;
    fixture.identity_provider.set_token_response(
        ISSUER,
        CLIENT_ID,
        "wrong-synthetic-nonce",
        CLIENT_ID,
    )?;
    let wrong_nonce_result = fixture.complete_login(wrong_nonce_login).await?;
    assert!(wrong_nonce_result.result.is_err());
    Ok(())
}

async fn assert_issuer_mismatch_is_rejected(fixture: &AuthenticationFixture) -> ProofResult<()> {
    let wrong_issuer_login = fixture.start_login().await?;
    fixture.identity_provider.set_token_response(
        "https://wrong-issuer.example.test",
        CLIENT_ID,
        &wrong_issuer_login.nonce,
        CLIENT_ID,
    )?;
    let wrong_issuer_result = fixture.complete_login(wrong_issuer_login).await?;
    assert!(wrong_issuer_result.result.is_err());
    Ok(())
}

async fn assert_audience_mismatch_is_rejected(fixture: &AuthenticationFixture) -> ProofResult<()> {
    let wrong_audience_login = fixture.start_login().await?;
    fixture.identity_provider.set_token_response(
        ISSUER,
        "another-synthetic-client",
        &wrong_audience_login.nonce,
        CLIENT_ID,
    )?;
    let wrong_audience_result = fixture.complete_login(wrong_audience_login).await?;
    assert!(wrong_audience_result.result.is_err());
    Ok(())
}

async fn assert_authorized_party_mismatch_is_rejected(
    fixture: &AuthenticationFixture,
) -> ProofResult<()> {
    let wrong_azp_login = fixture.start_login().await?;
    fixture.identity_provider.set_token_response(
        ISSUER,
        CLIENT_ID,
        &wrong_azp_login.nonce,
        "another-synthetic-client",
    )?;
    let wrong_azp_result = fixture.complete_login(wrong_azp_login).await?;
    let wrong_azp_token = wrong_azp_result
        .result
        .as_ref()
        .ok()
        .and_then(|completed| completed.id_token.as_ref())
        .ok_or_else(|| io::Error::other("Huskarl did not return the validated ID token"))?;
    assert!(validate_authorized_party(wrong_azp_token, CLIENT_ID).is_err());
    Ok(())
}

async fn assert_matching_flow_succeeds(fixture: &AuthenticationFixture) -> ProofResult<()> {
    let accepted_login = fixture.start_login().await?;
    fixture.identity_provider.set_token_response(
        ISSUER,
        CLIENT_ID,
        &accepted_login.nonce,
        CLIENT_ID,
    )?;
    let accepted_url = accepted_login.authorization_url.clone();
    let accepted_result = fixture.complete_login(accepted_login).await?;
    let accepted = accepted_result
        .result
        .as_ref()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let id_token = accepted
        .id_token
        .as_ref()
        .ok_or_else(|| io::Error::other("OIDC flow returned no ID token"))?;
    validate_authorized_party(id_token, CLIENT_ID)?;
    assert_pkce_round_trip(
        &accepted_url,
        &fixture.identity_provider.last_token_request()?,
    )?;
    Ok(())
}

#[tokio::test]
async fn cancelling_loopback_completion_releases_the_listener() -> ProofResult<()> {
    let fixture = AuthenticationFixture::new().await?;
    let login = fixture.start_login().await?;
    let callback_address = fixture.listener.local_addr()?;
    let AuthenticationFixture {
        grant, listener, ..
    } = fixture;
    let listener = Arc::try_unwrap(listener)
        .map_err(|_| io::Error::other("callback listener has more than one owner"))?;
    let completion_task = tokio::spawn(async move {
        grant
            .complete_on_loopback(&listener, &login.pending_state, None)
            .await
    });
    tokio::task::yield_now().await;
    completion_task.abort();
    assert!(completion_task.await.is_err());
    TcpListener::bind(callback_address).await?;
    Ok(())
}

#[tokio::test]
async fn reqwest_auth_adapter_returns_redirects_without_following_them() -> ProofResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let expected_location = format!("http://{address}/redirect-target");
    let response_location = expected_location.clone();
    let server = serve_redirect_once(listener, response_location);

    let client = async {
        let client = ReqwestClient::builder().build().await?;
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("http://{address}/token"))
            .body(Bytes::from_static(b"grant_type=authorization_code"))?;
        let response = client.execute(request, Idempotency::Unknown).await?;
        Ok::<_, ProofError>(response)
    };
    let ((), response) = tokio::try_join!(server, client)?;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(
        response
            .headers
            .get(LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some(expected_location.as_str())
    );
    Ok(())
}

async fn serve_redirect_once(listener: TcpListener, location: String) -> ProofResult<()> {
    let (mut stream, _) = listener.accept().await?;
    drop(listener);

    let mut request = [0_u8; 2048];
    if stream.read(&mut request).await? == 0 {
        return Err(io::Error::other("synthetic request was empty").into());
    }

    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
mod verifier_tests {
    use super::*;

    #[test]
    fn verifier_only_accepts_public_rs256_signing_keys() -> ProofResult<()> {
        let parsed_jwks: huskarl::core::jwk::Jwks = serde_json::from_str(JWKS_RESPONSE)?;
        let public_jwks: huskarl::core::jwk::PublicJwks = parsed_jwks.into();
        let rsa_key = public_jwks
            .keys
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("synthetic JWKS has no key"))?;
        assert!(jwk_is_acceptable(&rsa_key));
        assert_eq!(RS256_ALGORITHMS, &["RS256"]);
        assert!(!has_minimum_rsa_strength(&[0x7f; 256]));
        assert!(has_minimum_rsa_strength(&[0x80; 256]));

        let mut padded_2047_bit_modulus = vec![0];
        padded_2047_bit_modulus.extend([0x7f; 256]);
        assert!(!has_minimum_rsa_strength(&padded_2047_bit_modulus));

        let mut padded_2048_bit_modulus = vec![0];
        padded_2048_bit_modulus.extend([0x80; 256]);
        assert!(has_minimum_rsa_strength(&padded_2048_bit_modulus));

        let mut private_jwk = rsa_key.clone();
        private_jwk.has_private_parameters = true;
        assert!(!jwk_is_acceptable(&private_jwk));

        let mut encryption_jwk = rsa_key.clone();
        encryption_jwk.key_use = Some(KeyUse::Encrypt);
        assert!(!jwk_is_acceptable(&encryption_jwk));

        let mut unsupported_algorithm_jwk = rsa_key.clone();
        unsupported_algorithm_jwk.algorithm = Some("PS256".to_owned());
        assert!(!jwk_is_acceptable(&unsupported_algorithm_jwk));
        Ok(())
    }

    #[test]
    fn authorized_party_must_match_and_be_present_for_multiple_audiences() {
        let single_audience_without_azp = validated_token(&[CLIENT_ID], None);
        assert!(validate_authorized_party(&single_audience_without_azp, CLIENT_ID).is_ok());

        let mismatched_azp = validated_token(&[CLIENT_ID], Some("other-client"));
        assert!(validate_authorized_party(&mismatched_azp, CLIENT_ID).is_err());

        let multiple_audiences_without_azp =
            validated_token(&[CLIENT_ID, "trusted-resource"], None);
        assert!(validate_authorized_party(&multiple_audiences_without_azp, CLIENT_ID).is_err());
    }

    fn validated_token(
        audiences: &[&str],
        authorized_party: Option<&str>,
    ) -> ValidatedJwt<IdTokenClaims> {
        let mut claims = IdTokenClaims::default();
        claims.azp = authorized_party.map(str::to_owned);
        ValidatedJwt::builder()
            .aud(
                audiences
                    .iter()
                    .map(|audience| (*audience).to_owned())
                    .collect(),
            )
            .claims(claims)
            .build()
    }
}
