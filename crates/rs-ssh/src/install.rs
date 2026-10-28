//! Install the device key over an existing SSH session (D15).

use ssh_key::{Algorithm, PublicKey};
use thiserror::Error;

use crate::{ExecOutput, SshError, SshSession};

const MAX_STDERR: usize = 512;
const DEVICE_DELIMITER: &str = "RS_DEVICE_KEY_EOF";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallRequest {
    /// Device identifier: exactly 16 lowercase RFC 4648 base32 characters.
    pub device_id: String,
    /// Result of [`crate::public_key_openssh`] with `rs-device=<device_id>` as its comment.
    pub device_key_line: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStatus {
    Added,
    AlreadyPresent,
    Skipped(SkipReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    TailscaleSsh,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallReport {
    pub device: LineStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum InstallError {
    #[error("invalid SSH key install input: {0}")]
    InvalidInput(&'static str),
    #[error(transparent)]
    Ssh(#[from] SshError),
    #[error("SSH key install script failed with status {status:?}: {stderr}")]
    ScriptFailed { status: Option<u32>, stderr: String },
    #[error("invalid SSH key install script output")]
    BadOutput,
}

#[derive(Debug, PartialEq, Eq)]
struct ValidatedRequest {
    device_id: String,
    device_key_line: String,
    device_base64: String,
}

/// Build a POSIX `sh` program that installs the device key.
///
/// All user-controlled values are validated before they are embedded. Newlines,
/// quotes and shell metacharacters are therefore impossible in those values, and
/// the device key is written through a quoted heredoc.
pub fn build_script(req: &InstallRequest) -> Result<String, InstallError> {
    let valid = validate(req)?;

    if valid.device_key_line.contains(DEVICE_DELIMITER) {
        return Err(InstallError::InvalidInput(
            "an embedded value collides with a heredoc delimiter",
        ));
    }

    let device_key_line = &valid.device_key_line;
    let device_base64 = &valid.device_base64;

    Ok(format!(
        r#"#!/bin/sh
set -eu
umask 077
mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
ak="$HOME/.ssh/authorized_keys"
if [ ! -e "$ak" ]; then
    : > "$ak"
fi
chmod 600 "$ak"
if [ -s "$ak" ]; then
    last=$(tail -c 1 "$ak")
    if [ -n "$last" ]; then
        printf '\n' >> "$ak"
    fi
fi
device_b64='{device_base64}'
if grep -qF "$device_b64" "$ak"; then
    printf '%s\n' 'device=present'
else
    cat >> "$ak" <<'{DEVICE_DELIMITER}'
{device_key_line}
{DEVICE_DELIMITER}
    printf '%s\n' 'device=added'
fi
printf '%s\n' 'done'
"#
    ))
}

fn validate(req: &InstallRequest) -> Result<ValidatedRequest, InstallError> {
    if !valid_device_id(&req.device_id) {
        return Err(InstallError::InvalidInput(
            "device_id must be exactly 16 lowercase base32 characters",
        ));
    }

    if req.device_key_line.len() > 1024 {
        return Err(InstallError::InvalidInput(
            "device key line must be at most 1024 bytes",
        ));
    }
    let device_parts: Vec<_> = req.device_key_line.split(' ').collect();
    if device_parts.len() != 3 || device_parts.iter().any(|part| part.is_empty()) {
        return Err(InstallError::InvalidInput(
            "device key line must have exactly three fields",
        ));
    }
    let algorithm = match device_parts.first().copied() {
        Some("ecdsa-sha2-nistp256") | Some("ssh-ed25519") => device_parts[0],
        _ => {
            return Err(InstallError::InvalidInput(
                "device key must be ECDSA P-256 or Ed25519",
            ))
        }
    };
    if !valid_base64(device_parts[1]) {
        return Err(InstallError::InvalidInput(
            "device key base64 contains invalid characters",
        ));
    }
    let expected_comment = format!("rs-device={}", req.device_id);
    if device_parts[2] != expected_comment {
        return Err(InstallError::InvalidInput(
            "device key comment must be rs-device=<device_id>",
        ));
    }
    let parsed = PublicKey::from_openssh(&req.device_key_line)
        .map_err(|_| InstallError::InvalidInput("device key cannot be parsed"))?;
    if parsed.algorithm().as_str() != algorithm || !allowed_device_algorithm(parsed.algorithm()) {
        return Err(InstallError::InvalidInput(
            "device key algorithm does not match its key data",
        ));
    }
    if parsed.comment().as_str().ok() != Some(expected_comment.as_str()) {
        return Err(InstallError::InvalidInput(
            "device key comment must be rs-device=<device_id>",
        ));
    }

    Ok(ValidatedRequest {
        device_id: req.device_id.clone(),
        device_key_line: req.device_key_line.clone(),
        device_base64: device_parts[1].to_owned(),
    })
}

fn valid_device_id(device_id: &str) -> bool {
    device_id.len() == 16
        && device_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || matches!(byte, b'2'..=b'7'))
}

fn valid_base64(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

fn allowed_device_algorithm(algorithm: Algorithm) -> bool {
    matches!(
        algorithm,
        Algorithm::Ed25519
            | Algorithm::Ecdsa {
                curve: ssh_key::EcdsaCurve::NistP256
            }
    )
}

/// Parse the fixed `key=value` report emitted by [`build_script`].
pub fn parse_report(stdout: &str) -> Result<InstallReport, InstallError> {
    let mut device = None;
    let mut done = false;
    let mut lines = stdout.lines().peekable();
    while let Some(line) = lines.next() {
        if done || line.is_empty() {
            return Err(InstallError::BadOutput);
        }
        if line == "done" {
            if lines.peek().is_some() {
                return Err(InstallError::BadOutput);
            }
            done = true;
            continue;
        }
        let (key, value) = line.split_once('=').ok_or(InstallError::BadOutput)?;
        match key {
            "device" if device.is_none() => {
                device = Some(match value {
                    "added" => LineStatus::Added,
                    "present" => LineStatus::AlreadyPresent,
                    _ => return Err(InstallError::BadOutput),
                });
            }
            _ => return Err(InstallError::BadOutput),
        }
    }
    if !done {
        return Err(InstallError::BadOutput);
    }
    Ok(InstallReport {
        device: device.ok_or(InstallError::BadOutput)?,
    })
}

/// Install the device key through an existing authenticated session.
pub async fn install_keys(
    session: &SshSession,
    req: &InstallRequest,
) -> Result<InstallReport, InstallError> {
    if session.server_info().tailscale {
        return Ok(InstallReport {
            device: LineStatus::Skipped(SkipReason::TailscaleSsh),
        });
    }

    let script = build_script(req)?;
    tracing::debug!(script_len = script.len(), "running SSH key install script");
    let ExecOutput {
        status,
        stdout,
        stderr,
    } = session
        .exec_with_stdin("/bin/sh -s", script.as_bytes())
        .await?;
    if status != Some(0) {
        let stderr = truncate_stderr(&stderr);
        return Err(InstallError::ScriptFailed { status, stderr });
    }
    parse_report(&String::from_utf8_lossy(&stdout))
}

fn truncate_stderr(stderr: &[u8]) -> String {
    let end = stderr.len().min(MAX_STDERR);
    let mut text = String::from_utf8_lossy(&stderr[..end]).into_owned();
    while text.len() > MAX_STDERR {
        text.pop();
    }
    text
}
