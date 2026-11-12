mod credential;
mod ntlm;

#[cfg(test)]
pub(crate) use self::credential::decode_password;
pub(crate) use self::credential::{
    MAX_CREDENTIAL_BYTES, delete_password, load_password, save_password,
};
#[cfg(test)]
pub(crate) use self::ntlm::synthetic_type_two_challenge;
pub(crate) use self::ntlm::{NtlmHandshake, decode_challenge, encode_token};
