#!/usr/bin/env bash
# Smoke tests for all three private desktop layers.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
run_script="$script_dir/run.sh"
log_root="${RS_DESKTOP_LOG_DIR:-${TMPDIR:-/tmp}/rs-desktop-logs}"

check_process_groups() {
    local record_file
    local pgid
    local name
    local bad=0
    [[ -d "$log_root" ]] || return 0
    while IFS= read -r -d '' record_file; do
        while read -r pgid name; do
            [[ -n "$pgid" ]] || continue
            if ps -o pid= -g "$pgid" 2>/dev/null | awk 'NF { found = 1 } END { exit(found ? 0 : 1) }'; then
                printf 'process group remains after run: %s (%s)\n' "$pgid" "$name" >&2
                bad=1
            fi
        done < "$record_file"
    done < <(find "$log_root" -type f -name pgids -print0 2>/dev/null)
    (( bad == 0 ))
}

run_pipewire() {
    local output
    local status
    local groups_ok=0
    if output="$("$run_script" pipewire -- pw-cli ls Node 2>&1)"; then
        status=0
    else
        status=$?
    fi
    printf '%s\n' "$output"
    if check_process_groups; then
        groups_ok=1
    fi
    if (( status == 0 )) && [[ "$output" == *rs-null-sink* ]] && (( groups_ok )); then
        printf 'PASS pipewire\n'
        return 0
    fi
    printf 'FAIL pipewire\n'
    return 1
}

run_x11() {
    local output
    local status
    local groups_ok=0
    if output="$("$run_script" x11 -- xdpyinfo -queryExtensions 2>&1)"; then
        status=0
    else
        status=$?
    fi
    printf '%s\n' "$output"
    if check_process_groups; then
        groups_ok=1
    fi
    if (( status == 0 )) \
        && [[ "$output" == *XTEST* ]] \
        && [[ "$output" == *DAMAGE* ]] \
        && [[ "$output" == *XFIXES* ]] \
        && [[ "$output" == *RANDR* ]] \
        && [[ "$output" == *MIT-SHM* ]] \
        && [[ "$output" == *XInputExtension* ]] \
        && (( groups_ok )); then
        printf 'PASS x11\n'
        return 0
    fi
    printf 'FAIL x11\n'
    return 1
}

run_sway() {
    local output
    local status
    local groups_ok=0
    if output="$("$run_script" sway -- dbus-send --session --print-reply \
        --dest=org.freedesktop.portal.Desktop \
        /org/freedesktop/portal/desktop \
        org.freedesktop.DBus.Properties.Get \
        string:org.freedesktop.portal.ScreenCast string:version 2>&1)"; then
        status=0
    else
        status=$?
    fi
    printf '%s\n' "$output"
    if check_process_groups; then
        groups_ok=1
    fi
    if (( status == 0 )) && [[ "$output" == *uint32* ]] && (( groups_ok )); then
        printf 'PASS sway\n'
        return 0
    fi
    printf 'FAIL sway\n'
    return 1
}

overall=0
run_pipewire || overall=1
run_x11 || overall=1
run_sway || overall=1
exit "$overall"
