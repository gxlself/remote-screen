//! A single ProxyJump, with independent host-key verification and authentication.

use crate::{SshError, SshTarget};

pub(crate) fn validate(target: &SshTarget) -> Result<(), SshError> {
    if target.jump.as_ref().is_some_and(|jump| jump.jump.is_some()) {
        return Err(SshError::Unsupported("only one ProxyJump hop is supported"));
    }
    for target in std::iter::once(target).chain(target.jump.as_deref()) {
        if target.host.is_empty() || target.user.is_empty() || target.port == 0 {
            return Err(SshError::InvalidOptions(
                "host, user and a nonzero port are required",
            ));
        }
    }
    Ok(())
}
