use std::net::SocketAddr;

use reqwest::{
    StatusCode,
    header::{HeaderValue, WWW_AUTHENTICATE},
};
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};
use zeroize::{Zeroize, Zeroizing};

use super::MAX_NTLM_TOKEN_BYTES;
use crate::{auth::encode_token, error::EwsError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ConnectionIdentity {
    pub(super) local: SocketAddr,
    pub(super) remote: SocketAddr,
    pub(super) certificate_hash: [u8; 32],
}

pub(super) fn ntlm_header(token: &[u8]) -> Result<HeaderValue, EwsError> {
    if token.is_empty() || token.len() > MAX_NTLM_TOKEN_BYTES {
        return Err(EwsError::ResourceLimitExceeded);
    }
    let encoded = encode_token(token)?;
    let mut value = Zeroizing::new(format!("NTLM {}", encoded.as_str()));
    let mut header = HeaderValue::from_str(&value).map_err(|_| EwsError::InvalidResponse)?;
    header.set_sensitive(true);
    value.zeroize();
    Ok(header)
}

pub(super) fn ntlm_challenge(headers: &reqwest::header::HeaderMap) -> Result<&str, EwsError> {
    headers
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let (scheme, token) = value.split_once(' ')?;
            (scheme.eq_ignore_ascii_case("NTLM") && !token.trim().is_empty())
                .then_some(token.trim())
        })
        .ok_or(EwsError::AccessDenied)
}

pub(super) fn map_status(status: StatusCode) -> EwsError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => EwsError::AccessDenied,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT => EwsError::ServiceUnavailable,
        status if status.is_server_error() => EwsError::ServiceUnavailable,
        _ => EwsError::InvalidResponse,
    }
}

pub(super) fn tls_server_end_point(certificate_der: &[u8]) -> Result<Zeroizing<Vec<u8>>, EwsError> {
    let (_, certificate) = x509_parser::parse_x509_certificate(certificate_der)
        .map_err(|_| EwsError::InvalidResponse)?;
    let signature_algorithm = &certificate.signature_algorithm;
    let signature_oid = signature_algorithm.algorithm.to_id_string();
    let digest = if signature_oid == "1.2.840.113549.1.1.10" {
        use x509_parser::signature_algorithm::SignatureAlgorithm;

        let SignatureAlgorithm::RSASSA_PSS(parameters) =
            SignatureAlgorithm::try_from(signature_algorithm)
                .map_err(|_| EwsError::InvalidResponse)?
        else {
            return Err(EwsError::InvalidResponse);
        };
        digest_for_hash_oid(&parameters.hash_algorithm_oid().to_id_string())?
    } else {
        digest_for_signature_oid(&signature_oid)?
    };
    let mut binding = Zeroizing::new(b"tls-server-end-point:".to_vec());
    match digest {
        DigestAlgorithm::Sha256 => binding.extend_from_slice(&Sha256::digest(certificate_der)),
        DigestAlgorithm::Sha224 => binding.extend_from_slice(&Sha224::digest(certificate_der)),
        DigestAlgorithm::Sha384 => binding.extend_from_slice(&Sha384::digest(certificate_der)),
        DigestAlgorithm::Sha512 => binding.extend_from_slice(&Sha512::digest(certificate_der)),
    }
    Ok(binding)
}

pub(super) fn digest_for_signature_oid(signature_oid: &str) -> Result<DigestAlgorithm, EwsError> {
    match signature_oid {
        "1.2.840.113549.1.1.4"
        | "1.2.840.113549.1.1.5"
        | "1.2.840.10040.4.3"
        | "1.2.840.10045.4.1" => Ok(DigestAlgorithm::Sha256),
        "1.2.840.113549.1.1.14" | "1.2.840.10045.4.3.1" | "2.16.840.1.101.3.4.3.1" => {
            Ok(DigestAlgorithm::Sha224)
        }
        "1.2.840.113549.1.1.11" | "1.2.840.10045.4.3.2" | "2.16.840.1.101.3.4.3.2" => {
            Ok(DigestAlgorithm::Sha256)
        }
        "1.2.840.113549.1.1.12" | "1.2.840.10045.4.3.3" | "2.16.840.1.101.3.4.3.3" => {
            Ok(DigestAlgorithm::Sha384)
        }
        "1.2.840.113549.1.1.13" | "1.2.840.10045.4.3.4" | "2.16.840.1.101.3.4.3.4" => {
            Ok(DigestAlgorithm::Sha512)
        }
        _ => Err(EwsError::InvalidResponse),
    }
}

pub(super) fn digest_for_hash_oid(hash_oid: &str) -> Result<DigestAlgorithm, EwsError> {
    match hash_oid {
        "1.3.14.3.2.26" | "2.16.840.1.101.3.4.2.1" => Ok(DigestAlgorithm::Sha256),
        "2.16.840.1.101.3.4.2.4" => Ok(DigestAlgorithm::Sha224),
        "2.16.840.1.101.3.4.2.2" => Ok(DigestAlgorithm::Sha384),
        "2.16.840.1.101.3.4.2.3" => Ok(DigestAlgorithm::Sha512),
        _ => Err(EwsError::InvalidResponse),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DigestAlgorithm {
    Sha256,
    Sha224,
    Sha384,
    Sha512,
}
