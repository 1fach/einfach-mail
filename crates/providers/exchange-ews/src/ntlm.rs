use base64::{Engine, engine::general_purpose::STANDARD};
use secrecy::{ExposeSecret, SecretString};
use sspi::{
    AuthIdentity, AuthIdentityBuffers, BufferType, ClientRequestFlags, CredentialUse,
    DataRepresentation, Ntlm, SecurityBuffer, SecurityStatus, Sspi, SspiImpl, Username,
};
use zeroize::Zeroizing;

use crate::error::EwsError;

pub(crate) struct NtlmHandshake {
    ntlm: Ntlm,
    credentials: Option<AuthIdentityBuffers>,
}

impl NtlmHandshake {
    pub(crate) fn new(username: &str, password: &SecretString) -> Result<Self, EwsError> {
        let identity = AuthIdentity {
            username: Username::parse(username).map_err(|_| EwsError::InvalidConnectionDetails)?,
            password: password.expose_secret().to_owned().into(),
        };
        let mut ntlm = Ntlm::new();
        let credentials = ntlm
            .acquire_credentials_handle()
            .with_credential_use(CredentialUse::Outbound)
            .with_auth_data(&identity)
            .execute(&mut ntlm)
            .map_err(|_| EwsError::AccessDenied)?
            .credentials_handle;
        Ok(Self { ntlm, credentials })
    }

    pub(crate) fn initial_token(
        &mut self,
        target_name: &str,
    ) -> Result<Zeroizing<Vec<u8>>, EwsError> {
        self.token_for_challenge(target_name, None, None)
    }

    pub(crate) fn response_token(
        &mut self,
        target_name: &str,
        challenge: &[u8],
        channel_binding: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, EwsError> {
        self.token_for_challenge(target_name, Some(challenge), Some(channel_binding))
    }

    fn token_for_challenge(
        &mut self,
        target_name: &str,
        challenge: Option<&[u8]>,
        channel_binding: Option<&[u8]>,
    ) -> Result<Zeroizing<Vec<u8>>, EwsError> {
        if let Some(channel_binding) = channel_binding {
            self.ntlm.set_channel_bindings(channel_binding);
        }

        let mut output = vec![SecurityBuffer::new(Vec::new(), BufferType::Token)];
        let result = match challenge {
            Some(challenge) => {
                let mut input = vec![SecurityBuffer::new(challenge.to_vec(), BufferType::Token)];
                let mut builder = self
                    .ntlm
                    .initialize_security_context()
                    .with_credentials_handle(&mut self.credentials)
                    .with_context_requirements(ClientRequestFlags::empty())
                    .with_target_data_representation(DataRepresentation::Native)
                    .with_target_name(target_name)
                    .with_input(&mut input)
                    .with_output(&mut output);
                let mut generator = self
                    .ntlm
                    .initialize_security_context_impl(&mut builder)
                    .map_err(|_| EwsError::AccessDenied)?;
                generator
                    .resolve_to_result()
                    .map_err(|_| EwsError::AccessDenied)?
            }
            None => {
                let mut builder = self
                    .ntlm
                    .initialize_security_context()
                    .with_credentials_handle(&mut self.credentials)
                    .with_context_requirements(ClientRequestFlags::empty())
                    .with_target_data_representation(DataRepresentation::Native)
                    .with_target_name(target_name)
                    .with_output(&mut output);
                let mut generator = self
                    .ntlm
                    .initialize_security_context_impl(&mut builder)
                    .map_err(|_| EwsError::AccessDenied)?;
                generator
                    .resolve_to_result()
                    .map_err(|_| EwsError::AccessDenied)?
            }
        };

        if matches!(
            result.status,
            SecurityStatus::CompleteNeeded | SecurityStatus::CompleteAndContinue
        ) {
            self.ntlm
                .complete_auth_token(&mut output)
                .map_err(|_| EwsError::AccessDenied)?;
        }

        output
            .into_iter()
            .next()
            .map(|buffer| Zeroizing::new(buffer.buffer))
            .filter(|token| !token.is_empty())
            .ok_or(EwsError::InvalidResponse)
    }
}

pub(crate) fn encode_token(token: &[u8]) -> Result<Zeroizing<String>, EwsError> {
    if token.is_empty() || token.len() > 16 * 1024 {
        return Err(EwsError::ResourceLimitExceeded);
    }
    Ok(Zeroizing::new(STANDARD.encode(token)))
}

pub(crate) fn decode_challenge(value: &str) -> Result<Zeroizing<Vec<u8>>, EwsError> {
    let token = STANDARD
        .decode(value)
        .map_err(|_| EwsError::InvalidResponse)?;
    if !(32..=16 * 1024).contains(&token.len()) {
        return Err(EwsError::ResourceLimitExceeded);
    }
    Ok(Zeroizing::new(token))
}

#[cfg(test)]
pub(crate) fn synthetic_type_two_challenge() -> Vec<u8> {
    let mut challenge = Vec::with_capacity(60);
    challenge.extend_from_slice(b"NTLMSSP\0");
    challenge.extend_from_slice(&2_u32.to_le_bytes());
    challenge.extend_from_slice(&[0, 0, 0, 0, 56, 0, 0, 0]);
    challenge.extend_from_slice(&0xe288_82b7_u32.to_le_bytes());
    challenge.extend_from_slice(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
    challenge.extend_from_slice(&[0; 8]);
    challenge.extend_from_slice(&[4, 0, 4, 0, 56, 0, 0, 0]);
    challenge.extend_from_slice(&[6, 1, 0xb1, 0x1d, 0, 0, 0, 0x0f]);
    challenge.extend_from_slice(&[0; 4]);
    challenge
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;

    use super::{NtlmHandshake, synthetic_type_two_challenge};

    #[test]
    fn creates_ntlmv2_authenticate_data_with_channel_bindings() {
        let mut handshake =
            NtlmHandshake::new("EXAMPLE\\ada", &SecretString::from("synthetic-only"))
                .expect("synthetic NTLM credentials are valid");
        let negotiate = handshake
            .initial_token("exchange.example.test")
            .expect("NTLM negotiate token is created");
        assert_eq!(&negotiate[..12], b"NTLMSSP\0\x01\0\0\0");

        let authenticate = handshake
            .response_token(
                "exchange.example.test",
                &synthetic_type_two_challenge(),
                b"tls-server-end-point:synthetic-certificate-binding",
            )
            .expect("NTLM authenticate token is created");
        assert_eq!(&authenticate[..12], b"NTLMSSP\0\x03\0\0\0");

        let response_length = u16::from_le_bytes([authenticate[20], authenticate[21]]) as usize;
        let response_offset =
            u32::from_le_bytes(authenticate[24..28].try_into().expect("fixed NTLM field")) as usize;
        let response_end = response_offset + response_length;
        let ntlm_v2_response = authenticate
            .get(response_offset..response_end)
            .expect("NTLM response field is in the token");
        assert_eq!(&ntlm_v2_response[16..20], &[1, 1, 0, 0]);
        let target_info = ntlm_v2_response
            .get(44..)
            .expect("NTLMv2 target info follows its fixed fields");
        assert!(
            target_info
                .windows(4)
                .any(|pair| pair == [0x0a, 0, 0x10, 0])
        );
    }
}
