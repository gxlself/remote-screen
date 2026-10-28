//! Authentication policy derived from the current host configuration.

use crate::{auth::AuthPolicy, config::HostConfig};

/// Build the authentication policy used for a new connection.
pub fn auth_policy_from_config(config: &HostConfig) -> AuthPolicy {
    AuthPolicy {
        allowed_users: config.access.allowed_users.clone(),
        max_viewers: config.access.max_viewers,
        allow_remote_control: config.access.allow_remote_control,
        attended_mode: config.access.attended_mode,
        intranet_confirm_each: config.intranet.confirm_each,
        allow_intranet_pairing: config.intranet.allow_pairing,
        forced_denial: None,
    }
}

#[cfg(test)]
mod tests {
    use super::auth_policy_from_config;
    use crate::config::HostConfig;

    #[test]
    fn auth_policy_maps_every_config_field() {
        for enabled in [false, true] {
            let mut cfg = HostConfig::default();
            cfg.access.allowed_users = vec!["owner@example.com".into()];
            cfg.access.max_viewers = 3;
            cfg.access.allow_remote_control = enabled;
            cfg.access.attended_mode = !enabled;
            cfg.intranet.confirm_each = enabled;
            cfg.intranet.allow_pairing = !enabled;
            let policy = auth_policy_from_config(&cfg);
            assert_eq!(policy.allowed_users, cfg.access.allowed_users);
            assert_eq!(policy.max_viewers, 3);
            assert_eq!(policy.allow_remote_control, enabled);
            assert_eq!(policy.attended_mode, !enabled);
            assert_eq!(policy.intranet_confirm_each, enabled);
            assert_eq!(policy.allow_intranet_pairing, !enabled);
            assert_eq!(policy.forced_denial, None);
        }
    }
}
