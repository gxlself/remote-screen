//! 版本号解析与比较。
//!
//! The parser and ordering follow SemVer 2.0 precedence.
//! Numeric components are kept as strings because SemVer does not bound their
//! length. Build metadata is validated but not retained.

use std::cmp::Ordering;

/// A strict SemVer 2.0 version without build metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: String,
    pub minor: String,
    pub patch: String,
    pub prerelease: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VersionError {
    #[error("invalid semantic version")]
    Invalid,
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        numeric_compare(&self.major, &other.major)
            .then_with(|| numeric_compare(&self.minor, &other.minor))
            .then_with(|| numeric_compare(&self.patch, &other.patch))
            .then_with(|| prerelease_compare(&self.prerelease, &other.prerelease))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Parse a strict SemVer 2.0 version.
pub fn parse(v: &str) -> Result<Version, VersionError> {
    let (without_build, build) = match v.split_once('+') {
        Some((version, build)) => (version, Some(build)),
        None => (v, None),
    };
    if let Some(build) = build {
        validate_identifiers(build, false)?;
    }

    let (core, prerelease) = match without_build.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (without_build, None),
    };

    let mut core_parts = core.split('.');
    let major = core_parts.next().ok_or(VersionError::Invalid)?;
    let minor = core_parts.next().ok_or(VersionError::Invalid)?;
    let patch = core_parts.next().ok_or(VersionError::Invalid)?;
    if core_parts.next().is_some() {
        return Err(VersionError::Invalid);
    }
    for part in [major, minor, patch] {
        if !valid_numeric(part, true) {
            return Err(VersionError::Invalid);
        }
    }

    let prerelease = match prerelease {
        Some(value) => parse_identifiers(value, true)?,
        None => Vec::new(),
    };

    Ok(Version {
        major: major.to_owned(),
        minor: minor.to_owned(),
        patch: patch.to_owned(),
        prerelease,
    })
}

/// Compare two versions with SemVer 2.0 precedence, ignoring build metadata.
pub fn compare(a: &str, b: &str) -> Result<Ordering, VersionError> {
    Ok(parse(a)?.cmp(&parse(b)?))
}

fn validate_identifiers(value: &str, prerelease: bool) -> Result<(), VersionError> {
    parse_identifiers(value, prerelease).map(|_| ())
}

fn parse_identifiers(value: &str, prerelease: bool) -> Result<Vec<String>, VersionError> {
    let mut output = Vec::new();
    for identifier in value.split('.') {
        if identifier.is_empty()
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(VersionError::Invalid);
        }
        if prerelease && numeric(identifier) && !valid_numeric(identifier, true) {
            return Err(VersionError::Invalid);
        }
        output.push(identifier.to_owned());
    }
    Ok(output)
}

fn valid_numeric(value: &str, reject_leading_zero: bool) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && !(reject_leading_zero && value.len() > 1 && value.starts_with('0'))
}

fn numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn numeric_compare(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn prerelease_compare(a: &[String], b: &[String]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        (false, false) => {}
    }

    for (left, right) in a.iter().zip(b) {
        let ordering = match (numeric(left), numeric(right)) {
            (true, true) => numeric_compare(left, right),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => left.as_bytes().cmp(right.as_bytes()),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_without_retaining_build_metadata() {
        let version = parse("1.2.3-alpha.1+build.09").expect("valid version");
        let same = parse("1.2.3-alpha.1+other").expect("valid version");
        assert_eq!(version, same);
        assert_eq!(version.major, "1");
        assert_eq!(version.prerelease, ["alpha", "1"]);
    }
}
