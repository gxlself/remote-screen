//! 录制文件的密钥包裹接口与 AES-256-GCM 分块加解密（M3 V5）。
use std::fmt;

use zeroize::Zeroizing;

/// 文件密钥的包裹者。主密钥由系统钥匙串 / Android Keystore 保管，Rust 只拿到包裹后的不透明字节。
/// 实现方可能阻塞（钥匙串查询），只能在阻塞线程上调用。
pub trait SecretWrapper: Send + Sync {
    /// 包裹 32 字节文件密钥；输出 1..=255 字节，原样写进文件头。
    fn wrap(&self, key: &[u8; 32]) -> Result<Vec<u8>, WrapError>;
    /// 还原文件密钥。
    fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>, WrapError>;
    /// 写进文件头的包裹方式：1 = 原生回调（钥匙串 / Keystore）；0xF0..=0xFF 留给测试。
    fn kind(&self) -> u8;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapError {
    /// 没有主密钥或暂时拿不到（钥匙串锁着、原生回调没注册）。
    Unavailable,
    /// 数据被改，或主密钥不对。
    Rejected,
}

impl fmt::Display for WrapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WrapError::Unavailable => f.write_str("recording key unavailable"),
            WrapError::Rejected => f.write_str("recording key rejected"),
        }
    }
}

impl std::error::Error for WrapError {}

pub const KEY_LEN: usize = 32;
pub const TAG_LEN: usize = 16;

/// 一份录制的文件密钥。密钥内容不会出现在调试输出中，并在释放时清零。
pub struct FileKey(Zeroizing<[u8; KEY_LEN]>);

impl FileKey {
    pub fn random() -> Result<Self, getrandom::Error> {
        let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::fill(bytes.as_mut())?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: Zeroizing<[u8; KEY_LEN]>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for FileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileKey(..)")
    }
}

/// The nonce layout leaves the first four bytes reserved and encodes the chunk
/// index in network byte order.
pub fn chunk_nonce(index: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&index.to_be_bytes());
    nonce
}

fn aead_key(key: &FileKey) -> ring::aead::LessSafeKey {
    let unbound = match ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, key.as_bytes()) {
        Ok(unbound) => unbound,
        Err(_) => unreachable!("AES-256-GCM accepts a 32-byte key"),
    };
    ring::aead::LessSafeKey::new(unbound)
}

pub fn seal_chunk(key: &FileKey, index: u64, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let mut sealed = plaintext.to_vec();
    let nonce = ring::aead::Nonce::assume_unique_for_key(chunk_nonce(index));
    let aad = ring::aead::Aad::from(aad);
    match aead_key(key).seal_in_place_append_tag(nonce, aad, &mut sealed) {
        Ok(()) => sealed,
        Err(_) => unreachable!("AES-256-GCM sealing cannot fail for an in-memory buffer"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthFailed;

pub fn open_chunk(
    key: &FileKey,
    index: u64,
    aad: &[u8],
    sealed: &[u8],
) -> Result<Vec<u8>, AuthFailed> {
    if sealed.len() < TAG_LEN {
        return Err(AuthFailed);
    }
    let nonce = ring::aead::Nonce::assume_unique_for_key(chunk_nonce(index));
    let aad = ring::aead::Aad::from(aad);
    let mut opened = sealed.to_vec();
    let plaintext = aead_key(key)
        .open_in_place(nonce, aad, &mut opened)
        .map_err(|_| AuthFailed)?;
    Ok(plaintext.to_vec())
}

/// AES-GCM wrapper with a fixed test master key. Only tests may use it;
/// production adapters must provide their native SecretWrapper.
#[doc(hidden)]
pub mod testing {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{SecretWrapper, WrapError};
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
    use zeroize::Zeroizing;

    const VERSION: u8 = 1;
    const WRAPPED_LEN: usize = 1 + 12 + super::KEY_LEN + super::TAG_LEN;

    pub struct TestWrapper {
        master: [u8; super::KEY_LEN],
        available: AtomicBool,
    }

    impl TestWrapper {
        pub fn new(master_seed: u8) -> Self {
            Self {
                master: [master_seed; super::KEY_LEN],
                available: AtomicBool::new(true),
            }
        }

        pub fn set_available(&self, available: bool) {
            self.available.store(available, Ordering::Release);
        }

        fn key(&self) -> LessSafeKey {
            let unbound = match UnboundKey::new(&AES_256_GCM, &self.master) {
                Ok(unbound) => unbound,
                Err(_) => unreachable!("AES-256-GCM accepts a 32-byte key"),
            };
            LessSafeKey::new(unbound)
        }
    }

    impl SecretWrapper for TestWrapper {
        fn wrap(&self, key: &[u8; super::KEY_LEN]) -> Result<Vec<u8>, WrapError> {
            if !self.available.load(Ordering::Acquire) {
                return Err(WrapError::Unavailable);
            }
            let mut nonce_bytes = [0u8; 12];
            getrandom::fill(&mut nonce_bytes).map_err(|_| WrapError::Unavailable)?;
            let nonce = Nonce::assume_unique_for_key(nonce_bytes);
            let mut sealed = Zeroizing::new(key.to_vec());
            self.key()
                .seal_in_place_append_tag(nonce, Aad::from(&[VERSION][..]), &mut *sealed)
                .map_err(|_| WrapError::Rejected)?;
            let mut wrapped = Vec::with_capacity(WRAPPED_LEN);
            wrapped.push(VERSION);
            wrapped.extend_from_slice(&nonce_bytes);
            wrapped.extend_from_slice(&sealed);
            Ok(wrapped)
        }

        fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<[u8; super::KEY_LEN]>, WrapError> {
            if !self.available.load(Ordering::Acquire) {
                return Err(WrapError::Unavailable);
            }
            if wrapped.len() != WRAPPED_LEN || wrapped[0] != VERSION {
                return Err(WrapError::Rejected);
            }
            let mut nonce_bytes = [0u8; 12];
            nonce_bytes.copy_from_slice(&wrapped[1..13]);
            let nonce = Nonce::assume_unique_for_key(nonce_bytes);
            let mut sealed = Zeroizing::new(wrapped[13..].to_vec());
            let plaintext = self
                .key()
                .open_in_place(nonce, Aad::from(&[VERSION][..]), &mut sealed)
                .map_err(|_| WrapError::Rejected)?;
            if plaintext.len() != super::KEY_LEN {
                return Err(WrapError::Rejected);
            }
            let mut key = Zeroizing::new([0u8; super::KEY_LEN]);
            key.copy_from_slice(plaintext);
            Ok(key)
        }

        fn kind(&self) -> u8 {
            0xF0
        }
    }
}
