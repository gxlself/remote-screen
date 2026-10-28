#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/mac/uninstall.sh [--keep-data] [--keep-app] [--dry-run]

Unregister and stop Remote Host, remove /Applications/Remote Host.app, local data,
keychain identity, and Screen Recording/Accessibility grants for both bundle IDs.
--keep-data preserves data and keychain identity (and therefore existing pairing).
--dry-run prints every action without invoking the app or changing the system.
USAGE
}

keep_data=false
keep_app=false
dry_run=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --keep-data) keep_data=true; shift ;;
        --keep-app) keep_app=true; shift ;;
        --dry-run) dry_run=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; die "Unknown argument: $1" ;;
    esac
done
require_macos
[[ -n "${HOME:-}" && "$HOME" == /* && "$HOME" != / ]] || die 'Refusing an empty or unsafe home directory.'
uninstall_home="$(cd "$HOME" && pwd -P)"
[[ -n "$uninstall_home" && "$uninstall_home" != / ]] || die 'Refusing an unsafe home directory.'
installed_app="/Applications/$RS_MAC_APP_NAME"
ui="$installed_app/Contents/MacOS/Remote Host"
user_id="$(id -u)"

print_command() {
    printf '[mac-release] ' >&2
    printf '%q ' "$@" >&2
    printf '\n' >&2
}

best_effort() {
    print_command "$@"
    if [[ "$dry_run" != true ]] && ! "$@"; then
        warn 'Command did not succeed; continuing cleanup.'
    fi
}

remove_path() {
    local path="$1"
    [[ -n "$path" && "$path" == /* && "$path" != *'/../'* && "$path" != *'/./'* ]] \
        || die "Refusing unsafe removal path: $path"
    case "$path" in
        '/Applications/Remote Host.app'|\
        "$uninstall_home/Library/Application Support/Remote Host"|\
        "$uninstall_home/Library/Logs/Remote Host") ;;
        "$uninstall_home/Library/Caches/"ai.gxlself.remotehost*)
            # Wildcard matches must stay direct children of the expected folder.
            [[ "${path#"$uninstall_home/Library/Caches/"}" != */* ]] || die "Refusing unexpected path: $path" ;;
        "$uninstall_home/Library/Preferences/"ai.gxlself.remotehost*.plist)
            [[ "${path#"$uninstall_home/Library/Preferences/"}" != */* ]] || die "Refusing unexpected path: $path" ;;
        *) die "Refusing removal outside the Remote Host paths: $path" ;;
    esac
    print_command rm -rf -- "$path"
    if [[ "$dry_run" != true ]]; then
        rm -rf -- "$path"
    fi
}

log '1. Unregister the bundled LaunchAgent (or boot out its stable launchd label).'
if [[ -x "$ui" ]]; then
    print_command "$ui" --unregister-agent
    if [[ "$dry_run" != true ]] && ! "$ui" --unregister-agent; then
        warn 'SMAppService unregister failed; attempting launchctl bootout.'
        best_effort launchctl bootout "gui/$user_id/$RS_MAC_AGENT_ID"
    fi
else
    log "No installed UI executable at $ui; using launchctl fallback."
    best_effort launchctl bootout "gui/$user_id/$RS_MAC_AGENT_ID"
fi

log '2. Quit the Remote Host UI if it is running.'
best_effort osascript -e 'if application id "ai.gxlself.remotehost" is running then tell application id "ai.gxlself.remotehost" to quit'

log '3. Remove the installed app.'
if [[ "$keep_app" == true ]]; then
    log "Keeping $installed_app (--keep-app)."
else
    remove_path "$installed_app"
fi

log '4. Remove local data, caches, preferences, and logs.'
if [[ "$keep_data" == true ]]; then
    log 'Keeping local data, caches, preferences, and logs (--keep-data).'
else
    remove_path "$uninstall_home/Library/Application Support/Remote Host"
    log "Remove all matches: $uninstall_home/Library/Caches/ai.gxlself.remotehost*"
    log "Remove all matches: $uninstall_home/Library/Preferences/ai.gxlself.remotehost*.plist"
    shopt -s nullglob
    for path in "$uninstall_home"/Library/Caches/ai.gxlself.remotehost* \
        "$uninstall_home"/Library/Preferences/ai.gxlself.remotehost*.plist; do
        remove_path "$path"
    done
    remove_path "$uninstall_home/Library/Logs/Remote Host"
fi

log '5. Remove all Remote Host keychain identities (a new installation will need pairing again).'
if [[ "$keep_data" == true ]]; then
    log 'Keeping keychain identity and pairing (--keep-data).'
elif [[ "$dry_run" == true ]]; then
    print_command security delete-generic-password -s "$RS_MAC_UI_ID"
    log 'Repeat keychain deletion until no matching entry remains.'
else
    while true; do
        print_command security delete-generic-password -s "$RS_MAC_UI_ID"
        if security delete-generic-password -s "$RS_MAC_UI_ID" >/dev/null 2>&1; then
            log 'Deleted a Remote Host keychain entry; checking for another.'
        else
            security_status=$?
            # security exits 44 for errSecItemNotFound (-25300).
            if [[ "$security_status" != 44 ]]; then
                warn "Keychain deletion failed with status $security_status; check the keychain manually."
            fi
            break
        fi
    done
fi

log '6. Reset Screen Recording and Accessibility grants for the UI and the agent.'
for bundle_id in "$RS_MAC_UI_ID" "$RS_MAC_AGENT_ID"; do
    best_effort tccutil reset ScreenCapture "$bundle_id"
    best_effort tccutil reset Accessibility "$bundle_id"
done
if [[ "$dry_run" == true ]]; then
    log 'Dry run complete; no app, process, file, keychain entry, or permission was changed.'
else
    log 'Remote Host uninstall cleanup complete.'
fi
