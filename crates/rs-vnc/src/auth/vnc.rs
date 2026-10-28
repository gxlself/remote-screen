//! VNC Authentication, security type 2.

use aes::cipher::{Block, BlockCipherEncrypt, KeyInit};
use des::Des;
use tokio::io::{AsyncRead, AsyncReadExt};
use zeroize::Zeroize;

use crate::types::VncError;

/// Build the DES key used by VNC Authentication.
///
/// RFB takes at most the first eight password bytes, pads shorter passwords
/// with zeroes, and reverses the bit order in each byte.
pub fn des_key(password: &str) -> [u8; 8] {
    let mut key = [0u8; 8];
    for (dst, src) in key.iter_mut().zip(password.as_bytes().iter().take(8)) {
        *dst = src.reverse_bits();
    }
    key
}

/// Encrypt the two eight-byte challenge blocks with DES-ECB.
pub fn vnc_auth_response(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    let mut key = des_key(password);
    let cipher = Des::new_from_slice(&key).expect("DES key is always eight bytes");
    let mut response = *challenge;

    for block in response.chunks_exact_mut(8) {
        let mut encrypted = Block::<Des>::from([0u8; 8]);
        encrypted.copy_from_slice(block);
        cipher.encrypt_block(&mut encrypted);
        block.copy_from_slice(&encrypted);
    }

    key.zeroize();
    response
}

/// Verify a VNC Authentication response on the server side.
pub fn vnc_auth_verify(password: &str, challenge: &[u8; 16], response: &[u8; 16]) -> bool {
    vnc_auth_response(password, challenge) == *response
}

/// Read the fixed-size RFB VNC challenge.
pub async fn read_challenge<R: AsyncRead + Unpin>(r: &mut R) -> Result<[u8; 16], VncError> {
    let mut challenge = [0u8; 16];
    r.read_exact(&mut challenge).await?;
    Ok(challenge)
}
