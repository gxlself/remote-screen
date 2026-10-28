//! 版本清单解析、JCS 与 Ed25519 验签。

use std::fs::File;
use std::io::Read;
use std::path::Path;

use data_encoding::{BASE64URL_NOPAD, HEXLOWER_PERMISSIVE};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::de::{self, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::types::{Arch, Channel, HostOs};
use super::version;

pub const SIGNING_DOMAIN: &[u8] = b"rsp-release-v1\0";
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("invalid release manifest: {0}")]
    Invalid(&'static str),
    #[error("invalid release signature")]
    Signature,
    #[error("no trusted release keys")]
    NoTrustedKeys,
    #[error("release manifest is not applicable: {0}")]
    NotApplicable(&'static str),
}

impl ManifestError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "manifest",
            Self::Signature => "signature",
            Self::NoTrustedKeys => "no_keys",
            Self::NotApplicable(_) => "not_applicable",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "更新清单格式无效",
            Self::Signature => "更新清单签名无效",
            Self::NoTrustedKeys => "本机未配置更新公钥",
            Self::NotApplicable(_) => "更新清单不适用于本机",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    pub channel: Channel,
    pub os: HostOs,
    pub arch: Arch,
    pub version: String,
    pub url: String,
    pub sha256: [u8; 32],
    pub size: u64,
    pub notes: String,
    pub min_from: Option<String>,
    pub published_at: u64,
}

#[derive(Debug, Clone)]
pub struct TrustedKeys(Vec<VerifyingKey>);

impl TrustedKeys {
    pub fn from_base64url(keys: &[&str]) -> Result<Self, ManifestError> {
        let mut trusted = Vec::with_capacity(keys.len());
        for encoded in keys {
            trusted.push(
                decode_public_key(encoded).map_err(|_| ManifestError::Invalid("public_key"))?,
            );
        }
        Ok(Self(trusted))
    }

    pub fn from_csv(list: &str) -> Self {
        let mut trusted = Vec::new();
        for item in list.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            match decode_public_key(item) {
                Ok(key) => trusted.push(key),
                Err(_) => {
                    let prefix: String = item.chars().take(8).collect();
                    tracing::error!(public_key = %prefix, "ignoring invalid release public key");
                }
            }
        }
        Self(trusted)
    }

    pub fn builtin() -> Self {
        Self::from_csv(option_env!("RS_RELEASE_PUBKEYS").unwrap_or(""))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

struct StrictObject(Map<String, Value>);

impl<'de> Deserialize<'de> for StrictObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = StrictObject;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = Map::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if fields.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate key"));
                    }
                }
                Ok(StrictObject(fields))
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}

/// Build the exact message covered by the release signature.
///
/// Only `signature` is removed. An explicit `null` for any other field stays in
/// the canonical object, matching the release signer's `SigningBytes`.
pub fn signing_bytes(raw: &[u8]) -> Result<Vec<u8>, ManifestError> {
    let strict = parse_strict_object(raw)?;
    canonical_signing_bytes(strict.0)
}

pub fn verify_manifest(raw: &[u8], keys: &TrustedKeys) -> Result<VerifiedManifest, ManifestError> {
    let fields = parse_strict_object(raw)?.0;
    let manifest = validate_fields(&fields)?;
    let encoded_signature = string_field(&fields, "signature")?.to_owned();
    let message = canonical_signing_bytes(fields)?;

    let signature = decode_signature(&encoded_signature)?;
    if keys.is_empty() {
        return Err(ManifestError::NoTrustedKeys);
    }

    if !keys
        .0
        .iter()
        .any(|key| key.verify_strict(&message, &signature).is_ok())
    {
        return Err(ManifestError::Signature);
    }
    Ok(manifest)
}

pub fn check_applicable(
    manifest: &VerifiedManifest,
    current: &str,
    channel: &Channel,
    os: &HostOs,
    arch: &Arch,
) -> Result<(), ManifestError> {
    if manifest.channel != *channel || manifest.os != *os || manifest.arch != *arch {
        return Err(ManifestError::NotApplicable("selector"));
    }
    let current =
        version::parse(current).map_err(|_| ManifestError::NotApplicable("current_version"))?;
    let manifest_version =
        version::parse(&manifest.version).map_err(|_| ManifestError::Invalid("version"))?;
    if manifest_version <= current {
        return Err(ManifestError::NotApplicable("not_newer"));
    }
    if let Some(minimum) = &manifest.min_from {
        let minimum = version::parse(minimum).map_err(|_| ManifestError::Invalid("min_from"))?;
        if current < minimum {
            return Err(ManifestError::NotApplicable("min_from"));
        }
    }
    Ok(())
}

pub struct PackageHasher {
    context: ring::digest::Context,
    size: u64,
}

impl PackageHasher {
    pub fn new() -> Self {
        Self {
            context: ring::digest::Context::new(&ring::digest::SHA256),
            size: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.context.update(data);
        self.size += data.len() as u64;
    }

    pub fn finish(self) -> ([u8; 32], u64) {
        let digest = self.context.finish();
        let sha256 = digest
            .as_ref()
            .try_into()
            .expect("SHA-256 digests are always 32 bytes");
        (sha256, self.size)
    }
}

impl Default for PackageHasher {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("size mismatch")]
    Size { expected: u64, actual: u64 },
    #[error("sha256 mismatch")]
    Sha256,
}

pub fn verify_package_file(path: &Path, manifest: &VerifiedManifest) -> Result<(), PackageError> {
    let file = File::open(path)?;
    let actual = file.metadata()?.len();
    if actual != manifest.size {
        return Err(PackageError::Size {
            expected: manifest.size,
            actual,
        });
    }
    let mut file = file;
    let mut hasher = PackageHasher::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let (sha256, hashed_size) = hasher.finish();
    if hashed_size != manifest.size {
        return Err(PackageError::Size {
            expected: manifest.size,
            actual: hashed_size,
        });
    }
    if sha256 != manifest.sha256 {
        return Err(PackageError::Sha256);
    }
    Ok(())
}

fn parse_strict_object(raw: &[u8]) -> Result<StrictObject, ManifestError> {
    serde_json::from_slice::<StrictObject>(raw).map_err(|error| {
        if error.to_string().starts_with("duplicate key") {
            ManifestError::Invalid("duplicate_key")
        } else {
            ManifestError::Invalid("json")
        }
    })
}

fn canonical_signing_bytes(mut fields: Map<String, Value>) -> Result<Vec<u8>, ManifestError> {
    for value in fields.values() {
        match value {
            Value::String(_) | Value::Null => {}
            Value::Number(number) if safe_json_integer(number) => {}
            _ => return Err(ManifestError::Invalid("json")),
        }
    }
    fields.remove("signature");
    let canonical = serde_json_canonicalizer::to_vec(&Value::Object(fields))
        .map_err(|_| ManifestError::Invalid("json"))?;
    let mut output = Vec::with_capacity(SIGNING_DOMAIN.len() + canonical.len());
    output.extend_from_slice(SIGNING_DOMAIN);
    output.extend_from_slice(&canonical);
    Ok(output)
}

fn validate_fields(fields: &Map<String, Value>) -> Result<VerifiedManifest, ManifestError> {
    for name in [
        "channel",
        "os",
        "arch",
        "version",
        "url",
        "sha256",
        "size",
        "notes",
        "published_at",
        "signature",
    ] {
        match fields.get(name) {
            None | Some(Value::Null) => return Err(ManifestError::Invalid("missing_field")),
            Some(_) => {}
        }
    }

    for name in fields.keys() {
        if !matches!(
            name.as_str(),
            "channel"
                | "os"
                | "arch"
                | "version"
                | "url"
                | "sha256"
                | "size"
                | "notes"
                | "min_from"
                | "published_at"
                | "signature"
        ) {
            return Err(ManifestError::Invalid("unknown_field"));
        }
    }

    let channel = parse_channel(string_field(fields, "channel")?)?;
    let os = parse_os(string_field(fields, "os")?)?;
    let arch = parse_arch(string_field(fields, "arch")?)?;
    let manifest_version = string_field(fields, "version")?;
    version::parse(manifest_version).map_err(|_| ManifestError::Invalid("version"))?;
    let url = string_field(fields, "url")?;
    validate_url(url)?;
    let sha256 = decode_sha256(string_field(fields, "sha256")?)?;
    let size = integer_field(fields, "size")?;
    let notes = string_field(fields, "notes")?.to_owned();
    let min_from = match fields.get("min_from") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if value.is_empty() => None,
        Some(Value::String(value)) => {
            version::parse(value).map_err(|_| ManifestError::Invalid("min_from"))?;
            Some(value.clone())
        }
        Some(_) => return Err(ManifestError::Invalid("min_from")),
    };
    let published_at = integer_field(fields, "published_at")?;

    Ok(VerifiedManifest {
        channel,
        os,
        arch,
        version: manifest_version.to_owned(),
        url: url.to_owned(),
        sha256,
        size,
        notes,
        min_from,
        published_at,
    })
}

fn string_field<'a>(
    fields: &'a Map<String, Value>,
    name: &'static str,
) -> Result<&'a str, ManifestError> {
    match fields.get(name) {
        Some(Value::String(value)) => Ok(value),
        _ => Err(ManifestError::Invalid(name)),
    }
}

fn integer_field(fields: &Map<String, Value>, name: &'static str) -> Result<u64, ManifestError> {
    match fields.get(name) {
        Some(Value::Number(number)) => safe_integer(number).ok_or(ManifestError::Invalid(name)),
        _ => Err(ManifestError::Invalid(name)),
    }
}

fn safe_integer(number: &serde_json::Number) -> Option<u64> {
    if number.is_f64() {
        return None;
    }
    let maximum = MAX_SAFE_INTEGER as u64;
    if let Some(value) = number.as_u64() {
        return (value <= maximum).then_some(value);
    }
    let value = number.as_i64()?;
    (0..=MAX_SAFE_INTEGER)
        .contains(&value)
        .then_some(value as u64)
}

fn safe_json_integer(number: &serde_json::Number) -> bool {
    if number.is_f64() {
        return false;
    }
    if let Some(value) = number.as_u64() {
        return value <= MAX_SAFE_INTEGER as u64;
    }
    number
        .as_i64()
        .is_some_and(|value| (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value))
}

fn parse_channel(value: &str) -> Result<Channel, ManifestError> {
    match value {
        "stable" => Ok(Channel::Stable),
        "beta" => Ok(Channel::Beta),
        _ => Err(ManifestError::Invalid("channel")),
    }
}

fn parse_os(value: &str) -> Result<HostOs, ManifestError> {
    match value {
        "macos" => Ok(HostOs::Macos),
        "windows" => Ok(HostOs::Windows),
        "linux" => Ok(HostOs::Linux),
        _ => Err(ManifestError::Invalid("os")),
    }
}

fn parse_arch(value: &str) -> Result<Arch, ManifestError> {
    match value {
        "universal" => Ok(Arch::Universal),
        "x64" => Ok(Arch::X64),
        "arm64" => Ok(Arch::Arm64),
        _ => Err(ManifestError::Invalid("arch")),
    }
}

fn decode_sha256(value: &str) -> Result<[u8; 32], ManifestError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::Invalid("sha256"));
    }
    let lowercase = value.to_ascii_lowercase();
    let decoded = HEXLOWER_PERMISSIVE
        .decode(lowercase.as_bytes())
        .map_err(|_| ManifestError::Invalid("sha256"))?;
    decoded
        .try_into()
        .map_err(|_| ManifestError::Invalid("sha256"))
}

fn validate_url(value: &str) -> Result<(), ManifestError> {
    let Some((scheme, _)) = value.split_once(':') else {
        return Err(ManifestError::Invalid("url"));
    };
    let mut bytes = scheme.bytes();
    match bytes.next() {
        Some(byte) if byte.is_ascii_alphabetic() => {}
        _ => return Err(ManifestError::Invalid("url")),
    }
    if !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')) {
        return Err(ManifestError::Invalid("url"));
    }
    Ok(())
}

fn decode_signature(encoded: &str) -> Result<Signature, ManifestError> {
    let bytes = BASE64URL_NOPAD
        .decode(encoded.as_bytes())
        .map_err(|_| ManifestError::Signature)?;
    if BASE64URL_NOPAD.encode(&bytes) != encoded {
        return Err(ManifestError::Signature);
    }
    let bytes: [u8; 64] = bytes.try_into().map_err(|_| ManifestError::Signature)?;
    Ok(Signature::from_bytes(&bytes))
}

fn decode_public_key(encoded: &str) -> Result<VerifyingKey, ()> {
    let bytes = BASE64URL_NOPAD.decode(encoded.as_bytes()).map_err(|_| ())?;
    if BASE64URL_NOPAD.encode(&bytes) != encoded {
        return Err(());
    }
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| ())?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| ())
}
