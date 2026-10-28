//! Apple Remote Desktop DH authentication, security type 30.

use aes::cipher::{Block, BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use aes::Aes128;
use md5::Digest;
use num_bigint::BigUint;
use tokio::io::{AsyncRead, AsyncReadExt};
use zeroize::Zeroizing;

use crate::types::VncError;

const MAX_KEY_LEN: usize = 512;
const CREDENTIAL_FIELD_LEN: usize = 64;
const CREDENTIAL_MAX_LEN: usize = CREDENTIAL_FIELD_LEN - 1;
const PLAINTEXT_LEN: usize = 128;

/// Diffie-Hellman parameters sent by an ARD server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArdServerParams {
    pub generator: u16,
    pub key_len: u16,
    pub prime: Vec<u8>,
    pub server_public: Vec<u8>,
}

/// Read ARD's generator, key length, prime, and server public value.
pub async fn read_ard_params<R: AsyncRead + Unpin>(r: &mut R) -> Result<ArdServerParams, VncError> {
    let mut header = [0u8; 4];
    r.read_exact(&mut header).await?;

    let generator = u16::from_be_bytes([header[0], header[1]]);
    let key_len = u16::from_be_bytes([header[2], header[3]]);
    validate_header(generator, key_len)?;
    let key_len = usize::from(key_len);

    let mut prime = vec![0u8; key_len];
    r.read_exact(&mut prime).await?;
    let mut server_public = vec![0u8; key_len];
    r.read_exact(&mut server_public).await?;

    validate_values(&prime, &server_public)?;
    Ok(ArdServerParams {
        generator,
        key_len: key_len as u16,
        prime,
        server_public,
    })
}

/// Encode ARD parameters in the wire format (all fields are big-endian).
pub fn encode_ard_params(p: &ArdServerParams) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(4 + p.prime.len() + p.server_public.len());
    encoded.extend_from_slice(&p.generator.to_be_bytes());
    encoded.extend_from_slice(&p.key_len.to_be_bytes());
    encoded.extend_from_slice(&p.prime);
    encoded.extend_from_slice(&p.server_public);
    encoded
}

/// Produce a deterministic ARD response using caller-supplied private key and
/// fill bytes. The private exponent is copied into a zeroizing buffer because
/// the caller may retain its input slice.
pub fn ard_response_with(
    p: &ArdServerParams,
    user: &str,
    password: &str,
    private_key: &[u8],
    fill: &[u8; PLAINTEXT_LEN],
) -> Result<Vec<u8>, VncError> {
    let (key_len, prime, server_public) = validate_params(p)?;
    validate_credentials(user, password)?;

    let private_key = Zeroizing::new(private_key.to_vec());
    let exponent = BigUint::from_bytes_be(&private_key);
    // BigUint::modpow is not constant time; each connection gets a fresh key.
    let client_public = BigUint::from(p.generator).modpow(&exponent, &prime);
    // BigUint::modpow is not constant time; each connection gets a fresh key.
    let shared = server_public.modpow(&exponent, &prime);
    let client_public = left_pad(&client_public, key_len)?;
    let shared = Zeroizing::new(left_pad(&shared, key_len)?);
    let aes_key = md5_key(&shared);

    let mut plaintext = Zeroizing::new(fill.to_vec());
    let user_bytes = user.as_bytes();
    let password_bytes = password.as_bytes();
    plaintext[..user_bytes.len()].copy_from_slice(user_bytes);
    plaintext[user_bytes.len()] = 0;
    plaintext[CREDENTIAL_FIELD_LEN..CREDENTIAL_FIELD_LEN + password_bytes.len()]
        .copy_from_slice(password_bytes);
    plaintext[CREDENTIAL_FIELD_LEN + password_bytes.len()] = 0;

    aes_encrypt(&aes_key, &mut plaintext);

    let mut response = Vec::with_capacity(PLAINTEXT_LEN + key_len);
    response.extend_from_slice(&plaintext);
    response.extend_from_slice(&client_public);
    Ok(response)
}

/// Produce an ARD response with freshly generated private exponent and fill.
pub fn ard_response(p: &ArdServerParams, user: &str, password: &str) -> Result<Vec<u8>, VncError> {
    let key_len = validated_key_len(p)?;
    let mut private_key = Zeroizing::new(vec![0u8; key_len]);
    getrandom::fill(&mut private_key).map_err(|error| VncError::Io(format!("random: {error}")))?;
    let mut fill = Zeroizing::new(vec![0u8; PLAINTEXT_LEN]);
    getrandom::fill(&mut fill).map_err(|error| VncError::Io(format!("random: {error}")))?;

    let fill: &[u8; PLAINTEXT_LEN] = fill
        .as_slice()
        .try_into()
        .expect("random fill has the fixed plaintext length");
    ard_response_with(p, user, password, &private_key, fill)
}

/// Decrypt an ARD response on the server side.
pub fn ard_decrypt(
    p: &ArdServerParams,
    server_private: &[u8],
    response: &[u8],
) -> Result<(String, String), VncError> {
    let (key_len, prime, _) = validate_params(p)?;
    if response.len() != PLAINTEXT_LEN + key_len {
        return Err(protocol("ard response has invalid length"));
    }

    let client_public_bytes = &response[PLAINTEXT_LEN..];
    let client_public = BigUint::from_bytes_be(client_public_bytes);
    validate_public(&client_public, &prime)?;

    let server_private = Zeroizing::new(server_private.to_vec());
    let exponent = BigUint::from_bytes_be(&server_private);
    // BigUint::modpow is not constant time; each connection gets a fresh key.
    let shared_value = client_public.modpow(&exponent, &prime);
    let shared = Zeroizing::new(left_pad(&shared_value, key_len)?);
    let aes_key = md5_key(&shared);

    let mut plaintext = Zeroizing::new(response[..PLAINTEXT_LEN].to_vec());
    aes_decrypt(&aes_key, &mut plaintext);

    let user_end = plaintext[..CREDENTIAL_FIELD_LEN]
        .iter()
        .position(|&byte| byte == 0)
        .ok_or_else(|| protocol("ard user terminator missing"))?;
    let password_end = plaintext[CREDENTIAL_FIELD_LEN..]
        .iter()
        .position(|&byte| byte == 0)
        .ok_or_else(|| protocol("ard password terminator missing"))?;

    let user = String::from_utf8(plaintext[..user_end].to_vec())
        .map_err(|_| protocol("ard user is not utf-8"))?;
    let password = String::from_utf8(
        plaintext[CREDENTIAL_FIELD_LEN..CREDENTIAL_FIELD_LEN + password_end].to_vec(),
    )
    .map_err(|_| protocol("ard password is not utf-8"))?;
    Ok((user, password))
}

/// Compute the server's public Diffie-Hellman value and left-pad it to the
/// prime's byte length.
pub fn ard_server_public(generator: u16, prime: &[u8], server_private: &[u8]) -> Vec<u8> {
    let modulus = BigUint::from_bytes_be(prime);
    if prime.is_empty() || modulus == BigUint::from(0u8) {
        return Vec::new();
    }
    let exponent = BigUint::from_bytes_be(server_private);
    // BigUint::modpow is not constant time; each connection gets a fresh key.
    let public_value = BigUint::from(generator).modpow(&exponent, &modulus);
    pad_unchecked(&public_value, prime.len())
}

fn validate_header(generator: u16, key_len: u16) -> Result<(), VncError> {
    let key_len = usize::from(key_len);
    if key_len == 0 || key_len > MAX_KEY_LEN {
        return Err(protocol("ard key length out of range"));
    }
    if generator == 0 {
        return Err(protocol("ard generator is zero"));
    }
    Ok(())
}

fn validate_params(p: &ArdServerParams) -> Result<(usize, BigUint, BigUint), VncError> {
    validate_header(p.generator, p.key_len)?;
    let key_len = usize::from(p.key_len);
    if p.prime.len() != key_len || p.server_public.len() != key_len {
        return Err(protocol("ard parameter length mismatch"));
    }
    validate_values(&p.prime, &p.server_public)?;
    Ok((
        key_len,
        BigUint::from_bytes_be(&p.prime),
        BigUint::from_bytes_be(&p.server_public),
    ))
}

fn validated_key_len(p: &ArdServerParams) -> Result<usize, VncError> {
    validate_params(p).map(|(key_len, _, _)| key_len)
}

fn validate_values(prime_bytes: &[u8], public_bytes: &[u8]) -> Result<(), VncError> {
    let prime = BigUint::from_bytes_be(prime_bytes);
    if prime < BigUint::from(3u8) {
        return Err(protocol("ard prime is too small"));
    }
    let public = BigUint::from_bytes_be(public_bytes);
    validate_public(&public, &prime)
}

fn validate_public(public: &BigUint, prime: &BigUint) -> Result<(), VncError> {
    let minimum = BigUint::from(2u8);
    let maximum = prime - 2u8;
    if public < &minimum || public > &maximum {
        return Err(protocol("ard public value out of range"));
    }
    Ok(())
}

fn validate_credentials(user: &str, password: &str) -> Result<(), VncError> {
    if user.len() > CREDENTIAL_MAX_LEN
        || password.len() > CREDENTIAL_MAX_LEN
        || user.as_bytes().contains(&0)
        || password.as_bytes().contains(&0)
    {
        return Err(protocol("ard credentials too long"));
    }
    Ok(())
}

fn left_pad(value: &BigUint, width: usize) -> Result<Vec<u8>, VncError> {
    let bytes = value.to_bytes_be();
    if bytes.len() > width {
        return Err(protocol("ard value exceeds key length"));
    }
    let mut padded = vec![0u8; width];
    padded[width - bytes.len()..].copy_from_slice(&bytes);
    Ok(padded)
}

fn pad_unchecked(value: &BigUint, width: usize) -> Vec<u8> {
    let bytes = value.to_bytes_be();
    let mut padded = vec![0u8; width];
    if bytes.len() <= width {
        padded[width - bytes.len()..].copy_from_slice(&bytes);
    }
    padded
}

fn md5_key(shared: &[u8]) -> Zeroizing<Vec<u8>> {
    let digest: Digest = md5::compute(shared);
    Zeroizing::new(digest.0.to_vec())
}

fn aes_encrypt(key: &[u8], data: &mut [u8]) {
    let cipher = Aes128::new_from_slice(key).expect("MD5 always produces an AES-128 key");
    for block in data.chunks_exact_mut(16) {
        let mut encrypted = Block::<Aes128>::from([0u8; 16]);
        encrypted.copy_from_slice(block);
        cipher.encrypt_block(&mut encrypted);
        block.copy_from_slice(&encrypted);
    }
}

fn aes_decrypt(key: &[u8], data: &mut [u8]) {
    let cipher = Aes128::new_from_slice(key).expect("MD5 always produces an AES-128 key");
    for block in data.chunks_exact_mut(16) {
        let mut decrypted = Block::<Aes128>::from([0u8; 16]);
        decrypted.copy_from_slice(block);
        cipher.decrypt_block(&mut decrypted);
        block.copy_from_slice(&decrypted);
    }
}

fn protocol(message: &str) -> VncError {
    VncError::Protocol(message.to_owned())
}
