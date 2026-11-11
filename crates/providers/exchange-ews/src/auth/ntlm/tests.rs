use secrecy::SecretString;

use super::{NtlmHandshake, synthetic_type_two_challenge};

#[test]
fn creates_ntlmv2_authenticate_data_with_channel_bindings() {
    let mut handshake = NtlmHandshake::new("EXAMPLE\\ada", &SecretString::from("synthetic-only"))
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
