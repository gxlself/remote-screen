//! Parsers for macOS system query output (screen sharing, FileVault).

/// launchd job override values used by `launchctl print-disabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchdOverride {
    Enabled,
    Disabled,
}

/// The launchd label for the system screen-sharing service.
pub const SCREEN_SHARING_LABEL: &str = "com.apple.screensharing";

/// Parse the standard output of `fdesetup status`.
///
/// `FileVault is Off, but will be enabled after the next restart.` reports the
/// current state as off. Unknown or localized output yields `None`.
pub fn parse_fdesetup_status(stdout: &str) -> Option<bool> {
    for line in stdout.lines().map(str::trim) {
        if line.starts_with("FileVault is On") {
            return Some(true);
        }
        if line.starts_with("FileVault is Off") {
            return Some(false);
        }
    }
    None
}

/// Find an override for `label` in `launchctl print-disabled system` output.
pub fn parse_print_disabled(stdout: &str, label: &str) -> Option<LaunchdOverride> {
    let quoted_label = format!("\"{label}\"");
    let line = stdout
        .lines()
        .find(|line| line.contains(&quoted_label) && line.contains("=>"))?;
    let (_, value) = line.split_once("=>")?;
    match value.trim() {
        "disabled" | "true" => Some(LaunchdOverride::Disabled),
        "enabled" | "false" => Some(LaunchdOverride::Enabled),
        _ => None,
    }
}

/// Combine an explicit launchd override with whether the service is loaded.
pub fn screen_sharing_state(
    override_: Option<LaunchdOverride>,
    service_loaded: Option<bool>,
) -> Option<bool> {
    match override_ {
        Some(LaunchdOverride::Enabled) => Some(true),
        Some(LaunchdOverride::Disabled) => Some(false),
        None => service_loaded,
    }
}
