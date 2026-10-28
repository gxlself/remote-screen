//! 配对文件（`.rspair`）格式与口令派生。见 04「其他配对方式」。

use std::{collections::BTreeMap, fmt, net::SocketAddr};

use data_encoding::BASE64URL_NOPAD;
use zeroize::Zeroizing;

use crate::{
    ids::{fp_b64url, fp_from_b64url, id_from_fp},
    qr::{parse_address, percent_decode, percent_encode},
};

pub const PAIRFILE_MAX_BYTES: usize = 4096;
pub const PAIRFILE_LIFETIME_S: u64 = 24 * 60 * 60;
pub const PAIRFILE_PASSWORD_PREFIX: &[u8; 16] = b"rsp-pairfile-v1\0";

pub struct PairFile {
    pub v: u32,
    pub host_id: String,
    pub fp: [u8; 32],
    pub file_id: [u8; 16],
    pub secret: Zeroizing<[u8; 32]>,
    pub name: String,
    pub addrs: Vec<SocketAddr>,
    pub expires_at_s: u64,
}

impl fmt::Debug for PairFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairFile")
            .field("v", &self.v)
            .field("host_id", &self.host_id)
            .field("fp", &self.fp)
            .field("file_id", &self.file_id)
            .field("secret", &"<redacted>")
            .field("name", &self.name)
            .field("addrs", &self.addrs)
            .field("expires_at_s", &self.expires_at_s)
            .finish()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PairFileError {
    #[error("pairing file exceeds 4096 bytes")]
    TooLarge,
    #[error("expected rsp://pairfile with a query and no path or fragment")]
    Url,
    #[error("invalid query parameter or percent encoding")]
    Encoding,
    #[error("duplicate query parameter")]
    DuplicateParameter,
    #[error("missing query parameter: {0}")]
    MissingParameter(&'static str),
    #[error("unsupported pairing file version")]
    Version,
    #[error("invalid host fingerprint")]
    Fingerprint,
    #[error("host ID does not match the fingerprint")]
    HostId,
    #[error("file ID must be 16 bytes of unpadded base64url")]
    FileId,
    #[error("secret must be 32 bytes of unpadded base64url")]
    Secret,
    #[error("host name exceeds 64 characters")]
    Name,
    #[error("expected between one and eight IP addresses")]
    Addresses,
    #[error("invalid IP address or port")]
    Address,
    #[error("expiry must be a nonzero decimal unix timestamp in seconds")]
    Expiry,
}

/// Parse a URI or file text. Size is checked before trimming surrounding whitespace.
pub fn parse(text: &str) -> Result<PairFile, PairFileError> {
    if text.len() > PAIRFILE_MAX_BYTES {
        return Err(PairFileError::TooLarge);
    }
    let query = text
        .trim()
        .strip_prefix("rsp://pairfile?")
        .ok_or(PairFileError::Url)?;
    if query.contains('#') {
        return Err(PairFileError::Url);
    }
    let mut params = BTreeMap::new();
    for param in query.split('&') {
        let (key, value) = param.split_once('=').ok_or(PairFileError::Encoding)?;
        let key = percent_decode(key).map_err(|_| PairFileError::Encoding)?;
        // Query values include the secret; erase temporary decoded copies on every exit.
        let value = Zeroizing::new(percent_decode(value).map_err(|_| PairFileError::Encoding)?);
        if params.insert(key, value).is_some() {
            return Err(PairFileError::DuplicateParameter);
        }
    }
    let required = |key: &'static str| {
        params
            .get(key)
            .map(|value| value.as_str())
            .ok_or(PairFileError::MissingParameter(key))
    };
    if required("v")? != "1" {
        return Err(PairFileError::Version);
    }
    let fp = fp_from_b64url(required("fp")?).map_err(|_| PairFileError::Fingerprint)?;
    let host_id = required("h")?;
    if host_id != id_from_fp(&fp) {
        return Err(PairFileError::HostId);
    }
    let file_id = BASE64URL_NOPAD
        .decode(required("f")?.as_bytes())
        .map_err(|_| PairFileError::FileId)?
        .try_into()
        .map_err(|_| PairFileError::FileId)?;
    let secret = Zeroizing::new(
        BASE64URL_NOPAD
            .decode(required("s")?.as_bytes())
            .map_err(|_| PairFileError::Secret)?,
    );
    let secret = Zeroizing::new(
        secret
            .as_slice()
            .try_into()
            .map_err(|_| PairFileError::Secret)?,
    );
    let name = required("n")?;
    if name.chars().count() > 64 {
        return Err(PairFileError::Name);
    }
    let addresses: Vec<_> = required("a")?.split(',').collect();
    if !(1..=8).contains(&addresses.len()) || addresses.iter().any(|addr| addr.is_empty()) {
        return Err(PairFileError::Addresses);
    }
    let addrs = addresses
        .into_iter()
        .map(|value| parse_address(value).map_err(|_| PairFileError::Address))
        .collect::<Result<_, _>>()?;
    let expiry = required("e")?;
    if expiry.is_empty() || !expiry.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(PairFileError::Expiry);
    }
    let expires_at_s = expiry.parse::<u64>().map_err(|_| PairFileError::Expiry)?;
    if expires_at_s == 0 {
        return Err(PairFileError::Expiry);
    }
    Ok(PairFile {
        v: 1,
        host_id: host_id.to_owned(),
        fp,
        file_id,
        secret,
        name: name.to_owned(),
        addrs,
        expires_at_s,
    })
}

impl PairFile {
    /// Serialize without a trailing newline. Constructed values must satisfy `parse`'s limits.
    pub fn to_uri(&self) -> String {
        let addrs = self
            .addrs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let secret = Zeroizing::new(BASE64URL_NOPAD.encode(self.secret.as_ref()));
        format!(
            "rsp://pairfile?v={}&h={}&fp={}&f={}&s={}&n={}&a={}&e={}",
            self.v,
            percent_encode(&self.host_id),
            fp_b64url(&self.fp),
            BASE64URL_NOPAD.encode(&self.file_id),
            secret.as_str(),
            percent_encode(&self.name),
            addrs,
            self.expires_at_s,
        )
    }

    pub fn is_expired(&self, now_us: u64) -> bool {
        now_us / 1_000_000 >= self.expires_at_s
    }
}

pub fn spake_password(file_id: &[u8; 16], secret: &[u8; 32]) -> Zeroizing<[u8; 64]> {
    let mut password = Zeroizing::new([0; 64]);
    password[..16].copy_from_slice(PAIRFILE_PASSWORD_PREFIX);
    password[16..32].copy_from_slice(file_id);
    password[32..].copy_from_slice(secret);
    password
}

/// Produce a portable filename using the export time in UTC, measured in unix seconds.
pub fn file_name(host_name: &str, unix_s: u64) -> String {
    let sanitized: String = host_name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '-'
            } else {
                ch
            }
        })
        .collect();
    let name = match sanitized.trim() {
        "" => "Remote-Host",
        name => name,
    };
    // Howard Hinnant's civil_from_days (inverse of days_from_civil). Unix timestamps
    // here are nonnegative; even u64::MAX seconds fits these day/year calculations.
    let days = unix_s / 86_400 + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    year += u64::from(month <= 2);
    let hour = unix_s / 3600 % 24;
    let minute = unix_s / 60 % 60;
    let suffix = format!("-{year:04}{month:02}{day:02}-{hour:02}{minute:02}.rspair");
    let name: String = name.chars().take(80 - suffix.chars().count()).collect();
    name + &suffix
}
