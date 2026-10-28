//! Safe tmux session command generation.

use crate::SshError;

/// IDs are deliberately restricted so the command can be passed to a login shell.
pub fn command(id: &str) -> Result<String, SshError> {
    if id.is_empty()
        || id.len() > 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(SshError::InvalidOptions(
            "tmux id must match [A-Za-z0-9_-]{1,32}",
        ));
    }
    Ok(format!("tmux new -A -s rs-{id}"))
}
