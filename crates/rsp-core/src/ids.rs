//! Stable identifiers and human-readable certificate fingerprints.

use data_encoding::{BASE32_NOPAD, BASE64URL_NOPAD};

const CROCKFORD_BASE32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const RFC4648_BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Both device and host IDs use the first 80 bits of the certificate fingerprint.
pub fn id_from_fp(fp: &[u8; 32]) -> String {
    BASE32_NOPAD.encode(&fp[..10]).to_ascii_lowercase()
}

/// Display the first 80 bits as 16 uppercase Crockford base32 characters,
/// grouped into four four-character blocks.
pub fn fp_display(fp: &[u8; 32]) -> String {
    let encoded = BASE32_NOPAD.encode(&fp[..10]);
    let mut display = String::with_capacity(19);
    for (index, byte) in encoded.bytes().enumerate() {
        if index > 0 && index % 4 == 0 {
            display.push(' ');
        }
        let position = RFC4648_BASE32
            .iter()
            .position(|&candidate| candidate == byte)
            .expect("BASE32_NOPAD emits only RFC 4648 base32 characters");
        display.push(CROCKFORD_BASE32[position] as char);
    }
    display
}

/// Decode an unpadded base64url fingerprint and display its first 80 bits.
pub fn fp_display_b64url(value: &str) -> Result<String, FingerprintError> {
    fp_from_b64url(value).map(|fp| fp_display(&fp))
}

pub fn fp_b64url(fp: &[u8; 32]) -> String {
    BASE64URL_NOPAD.encode(fp)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FingerprintError {
    #[error("invalid unpadded base64url fingerprint")]
    Encoding(#[from] data_encoding::DecodeError),
    #[error("a certificate fingerprint must contain exactly 32 bytes")]
    Length,
}

pub fn fp_from_b64url(value: &str) -> Result<[u8; 32], FingerprintError> {
    BASE64URL_NOPAD
        .decode(value.as_bytes())?
        .try_into()
        .map_err(|_| FingerprintError::Length)
}
