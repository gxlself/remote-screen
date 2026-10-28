//! 配对二维码解析与生成。见 docs/dev/04-identity-security.md 配对。

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use crate::ids::{fp_b64url, fp_from_b64url, id_from_fp, FingerprintError};
use crate::DEFAULT_PORT;

/// The QR format currently carries IP literals only, without DNS resolution.
pub type SocketAddrOrHost = SocketAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairQr {
    pub v: u32,
    pub host_id: String,
    pub fp: [u8; 32],
    pub code: String,
    pub name: String,
    pub addrs: Vec<SocketAddrOrHost>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum QrError {
    #[error("expected rsp://pair with a query and no path or fragment")]
    Url,
    #[error("invalid query parameter or percent encoding")]
    Encoding,
    #[error("duplicate query parameter")]
    DuplicateParameter,
    #[error("missing query parameter: {0}")]
    MissingParameter(&'static str),
    #[error("unsupported pairing QR version")]
    Version,
    #[error(transparent)]
    Fingerprint(#[from] FingerprintError),
    #[error("host ID does not match the fingerprint")]
    HostId,
    #[error("pairing code must contain exactly six ASCII digits")]
    Code,
    #[error("host name exceeds 64 characters")]
    Name,
    #[error("expected between one and eight IP addresses")]
    Addresses,
    #[error("invalid IP address or port")]
    Address,
}

/// Parse a pairing URL, binding its host ID to its full fingerprint.
pub fn parse(value: &str) -> Result<PairQr, QrError> {
    let query = value.strip_prefix("rsp://pair?").ok_or(QrError::Url)?;
    if query.contains('#') {
        return Err(QrError::Url);
    }
    let mut params = BTreeMap::new();
    for param in query.split('&') {
        let (key, value) = param.split_once('=').ok_or(QrError::Encoding)?;
        let key = percent_decode(key)?;
        let value = percent_decode(value)?;
        // Include unknown parameters so that duplicates cannot hide in extensions.
        if params.insert(key, value).is_some() {
            return Err(QrError::DuplicateParameter);
        }
    }
    let required = |key: &'static str| {
        params
            .get(key)
            .map(String::as_str)
            .ok_or(QrError::MissingParameter(key))
    };
    if required("v")? != "1" {
        return Err(QrError::Version);
    }
    let fp = fp_from_b64url(required("fp")?)?;
    let host_id = required("h")?;
    if host_id != id_from_fp(&fp) {
        return Err(QrError::HostId);
    }
    let code = required("c")?;
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(QrError::Code);
    }
    let name = required("n")?;
    if name.chars().count() > 64 {
        return Err(QrError::Name);
    }
    let addresses: Vec<_> = required("a")?.split(',').collect();
    if !(1..=8).contains(&addresses.len()) || addresses.iter().any(|addr| addr.is_empty()) {
        return Err(QrError::Addresses);
    }
    let addrs = addresses
        .into_iter()
        .map(parse_address)
        .collect::<Result<_, _>>()?;
    Ok(PairQr {
        v: 1,
        host_id: host_id.to_owned(),
        fp,
        code: code.to_owned(),
        name: name.to_owned(),
        addrs,
    })
}

impl PairQr {
    pub fn parse(value: &str) -> Result<Self, QrError> {
        parse(value)
    }

    /// Serialize a QR value. Manually constructed values must satisfy `parse`'s limits.
    pub fn to_url(&self) -> String {
        let addrs = self
            .addrs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "rsp://pair?v={}&h={}&fp={}&c={}&n={}&a={}",
            self.v,
            percent_encode(&self.host_id),
            fp_b64url(&self.fp),
            percent_encode(&self.code),
            percent_encode(&self.name),
            addrs
        )
    }
}

pub(crate) fn parse_address(value: &str) -> Result<SocketAddr, QrError> {
    if let Ok(addr) = value.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Some(ip) = value.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return ip
            .parse::<std::net::Ipv6Addr>()
            .map(|ip| SocketAddr::new(IpAddr::V6(ip), DEFAULT_PORT))
            .map_err(|_| QrError::Address);
    }
    value
        .parse::<std::net::Ipv4Addr>()
        .map(|ip| SocketAddr::new(IpAddr::V4(ip), DEFAULT_PORT))
        .map_err(|_| QrError::Address)
}

pub(crate) fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    encoded
}

pub(crate) fn percent_decode(value: &str) -> Result<String, QrError> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = hex_digit(bytes.next().ok_or(QrError::Encoding)?)?;
            let low = hex_digit(bytes.next().ok_or(QrError::Encoding)?)?;
            decoded.push(high * 16 + low);
        } else {
            // '+' is literal in an RFC 3986 URI; it is not a form-encoded space.
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).map_err(|_| QrError::Encoding)
}

fn hex_digit(byte: u8) -> Result<u8, QrError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(QrError::Encoding),
    }
}
