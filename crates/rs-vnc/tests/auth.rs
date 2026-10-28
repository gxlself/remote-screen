use rs_vnc::auth::ard::{
    ard_decrypt, ard_response, ard_response_with, ard_server_public, encode_ard_params,
    read_ard_params, ArdServerParams,
};
use rs_vnc::auth::vnc::{des_key, read_challenge, vnc_auth_response, vnc_auth_verify};
use rs_vnc::VncError;
use sha2::{Digest, Sha256};
use tokio::io::{duplex, AsyncWriteExt};

const CHALLENGE: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];

fn hex(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2));
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).unwrap();
            let low = (pair[1] as char).to_digit(16).unwrap();
            ((high << 4) | low) as u8
        })
        .collect()
}

fn hex_array<const N: usize>(value: &str) -> [u8; N] {
    hex(value).try_into().unwrap()
}

fn assert_protocol<T: std::fmt::Debug>(result: Result<T, VncError>) {
    assert!(matches!(result, Err(VncError::Protocol(_))), "{result:?}");
}

async fn read_params(bytes: &[u8]) -> Result<ArdServerParams, VncError> {
    let (mut writer, mut reader) = duplex(bytes.len().max(64));
    writer.write_all(bytes).await.unwrap();
    drop(writer);
    read_ard_params(&mut reader).await
}

fn params64() -> ArdServerParams {
    ArdServerParams {
        generator: 2,
        key_len: 8,
        prime: hex("ffffffffffffffc5"),
        server_public: hex("4ae1c2fc845d127e"),
    }
}

fn params1024() -> (ArdServerParams, Vec<u8>, Vec<u8>) {
    let prime = hex(
        "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE65381FFFFFFFFFFFFFFFF",
    );
    let client_private = hex("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    let server_private = hex("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210");
    let server_public = ard_server_public(2, &prime, &server_private);
    (
        ArdServerParams {
            generator: 2,
            key_len: 128,
            prime,
            server_public,
        },
        client_private,
        server_private,
    )
}

#[test]
fn vnc_des_key() {
    assert_eq!(des_key("secret"), hex_array("cea6c64ea62e0000"));
    assert_eq!(des_key("password123"), hex_array("0e86ceceeef64e26"));
    assert_eq!(des_key(""), hex_array("0000000000000000"));
}

#[test]
fn vnc_response_vectors() {
    assert_eq!(
        vnc_auth_response("secret", &CHALLENGE),
        hex_array("ee22539f33a5983ec12f9c2edbc995dd")
    );
    assert_eq!(
        vnc_auth_response("password123", &CHALLENGE),
        hex_array("b866924125c8eebb9debc1db61c538e2")
    );
    assert_eq!(
        vnc_auth_response("", &CHALLENGE),
        hex_array("491e890de9ace932838a49792f2213f3")
    );
}

#[test]
fn vnc_long_password_truncated() {
    assert_eq!(
        vnc_auth_response("password123", &CHALLENGE),
        vnc_auth_response("password", &CHALLENGE)
    );
}

#[test]
fn vnc_verify() {
    let response = vnc_auth_response("secret", &CHALLENGE);
    assert!(vnc_auth_verify("secret", &CHALLENGE, &response));
    assert!(!vnc_auth_verify("secreT", &CHALLENGE, &response));
}

#[tokio::test]
async fn read_challenge_eof() {
    let (mut writer, mut reader) = duplex(32);
    writer.write_all(&CHALLENGE[..15]).await.unwrap();
    drop(writer);
    assert_eq!(
        read_challenge(&mut reader).await,
        Err(VncError::Closed("eof".into()))
    );
}

#[test]
fn ard64_vector() {
    let params = params64();
    let client_private = hex("1234567890abcdef");
    let server_private = hex("0fedcba987654321");
    assert_eq!(
        ard_server_public(2, &params.prime, &server_private),
        hex("4ae1c2fc845d127e")
    );
    assert_eq!(
        encode_ard_params(&params),
        hex("00020008ffffffffffffffc54ae1c2fc845d127e")
    );

    let response =
        ard_response_with(&params, "alice", "s3cret!", &client_private, &[0xaa; 128]).unwrap();
    let expected = hex(
        "3575c3724e68cc66c23d86c8912ea7aeaa3a244c5a3a48663df5b86d53019da7aa3a244c5a3a48663df5b86d53019da7aa3a244c5a3a48663df5b86d53019da77a55cbc66e04c41f7277d06566b6b198aa3a244c5a3a48663df5b86d53019da7aa3a244c5a3a48663df5b86d53019da7aa3a244c5a3a48663df5b86d53019da7be2345063365390e",
    );
    assert_eq!(response, expected);

    let mut hasher = Sha256::new();
    hasher.update(&response);
    assert_eq!(
        hasher.finalize().as_slice(),
        hex("dfa05cc1e1a06afd0e48c459b0b2df12efcd2e08d510041a954ffdfa5e339027")
    );
    assert_eq!(
        ard_decrypt(&params, &server_private, &response).unwrap(),
        ("alice".into(), "s3cret!".into())
    );
}

#[test]
fn ard64_leading_zero_secret() {
    let mut params = params64();
    let client_private = hex("1234567890abcdef");
    let server_private = [124u8];
    params.server_public = ard_server_public(2, &params.prime, &server_private);
    assert_eq!(
        ard_server_public(2, &params.prime, &server_private),
        hex("b0000000000000b1")
    );
    let response = ard_response_with(&params, "bob", "x", &client_private, &[0; 128]).unwrap();
    let expected = hex(
        "6e516231178b96252a59acc085bb4608772cceff0f05bad8a6a82bdd4fc880b7772cceff0f05bad8a6a82bdd4fc880b7772cceff0f05bad8a6a82bdd4fc880b7c0e84a7f4e69450c15f3e866914a3ba6772cceff0f05bad8a6a82bdd4fc880b7772cceff0f05bad8a6a82bdd4fc880b7772cceff0f05bad8a6a82bdd4fc880b7be2345063365390e",
    );
    assert_eq!(response, expected);
    let mut hasher = Sha256::new();
    hasher.update(&response);
    assert_eq!(
        hasher.finalize().as_slice(),
        hex("bf8ba96569aac09242978ce53761b76ff45362085d064b6b77eae62550f9fbcc")
    );
}

#[test]
fn ard1024_vector() {
    let (params, client_private, server_private) = params1024();
    assert_eq!(
        &params.server_public[..16],
        &hex("06377c99288b235eb159a163b9c7f2ac")
    );
    let response =
        ard_response_with(&params, "alice", "s3cret!", &client_private, &[0xaa; 128]).unwrap();
    assert_eq!(response.len(), 256);
    assert_eq!(
        &response[..32],
        &hex("8b3d3d20fee0832b94e9540c4a4c06470ab1de1378e6befb91e5cd46cfa382c7")
    );
    assert_eq!(
        &response[128..144],
        &hex("8087115ed28670eee8569b39819c1e00")
    );
    let mut hasher = Sha256::new();
    hasher.update(&response);
    assert_eq!(
        hasher.finalize().as_slice(),
        hex("3cff1b9317ea80689f4104dcf047aa5fef337cce7ce86b37107526b7e031ffbb")
    );
    assert_eq!(
        ard_decrypt(&params, &server_private, &response).unwrap(),
        ("alice".into(), "s3cret!".into())
    );
}

#[test]
fn ard_random_roundtrip() {
    let (params, _, server_private) = params1024();
    let first = ard_response(&params, "alice", "pw").unwrap();
    let second = ard_response(&params, "alice", "pw").unwrap();
    assert_ne!(first, second);
    assert_eq!(
        ard_decrypt(&params, &server_private, &first).unwrap(),
        ("alice".into(), "pw".into())
    );
    assert_eq!(
        ard_decrypt(&params, &server_private, &second).unwrap(),
        ("alice".into(), "pw".into())
    );
}

#[test]
fn ard_limits() {
    let params = params64();
    let private = hex("1234567890abcdef");
    assert_protocol(ard_response_with(
        &params,
        &"u".repeat(64),
        "pw",
        &private,
        &[0; 128],
    ));
    assert!(ard_response_with(&params, "u", &"p".repeat(63), &private, &[0; 128],).is_ok());
    assert_protocol(ard_response_with(
        &params,
        "u",
        &"p".repeat(64),
        &private,
        &[0; 128],
    ));
    assert_protocol(ard_response_with(
        &params, "u\0x", "pw", &private, &[0; 128],
    ));
}

#[tokio::test]
async fn ard_params_validation() {
    assert_protocol(read_params(&[0, 2, 0, 0]).await);
    assert_protocol(read_params(&[0, 2, 2, 1]).await);

    let mut generator_zero = vec![0, 0, 0, 8];
    generator_zero.extend_from_slice(&[0; 16]);
    assert_protocol(read_params(&generator_zero).await);

    let mut public_one = vec![0, 2, 0, 8];
    public_one.extend_from_slice(&hex("ffffffffffffffc5"));
    public_one.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    assert_protocol(read_params(&public_one).await);

    let mut public_p_minus_one = vec![0, 2, 0, 8];
    public_p_minus_one.extend_from_slice(&hex("ffffffffffffffc5"));
    public_p_minus_one.extend_from_slice(&hex("ffffffffffffffc4"));
    assert_protocol(read_params(&public_p_minus_one).await);

    let mut truncated_prime = vec![0, 2, 0, 8];
    truncated_prime.extend_from_slice(&[0; 3]);
    assert_eq!(
        read_params(&truncated_prime).await,
        Err(VncError::Closed("eof".into()))
    );
}

#[test]
fn ard_decrypt_bad_length() {
    let params = params64();
    assert_protocol(ard_decrypt(&params, &[1], &[0; 135]));
}

#[tokio::test]
async fn ard_params_roundtrip() {
    let params = params64();
    let encoded = encode_ard_params(&params);
    assert_eq!(read_params(&encoded).await.unwrap(), params);
}
