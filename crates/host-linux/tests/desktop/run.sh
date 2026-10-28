#!/usr/bin/env bash
# Desktop integration-test launcher for PipeWire, X11, and headless sway.
#
# Rust desktop tests must assert their layer at the start of each it_* test:
#   assert_eq!(std::env::var("RS_IT_LAYER").as_deref(), Ok("x11"));
# On a mismatch they panic with:
#   panic!("run under crates/host-linux/tests/desktop/run.sh <layer>")
# For example: crates/host-linux/tests/desktop/run.sh x11 -- cargo nextest run -p host-linux --features desktop-it --test it_x11_capture
set -euo pipefail

usage() {
    printf 'Usage: %s <pipewire|x11|sway> -- <command...>\n' "$0" >&2
}

if [[ $# -lt 3 || $2 != -- ]]; then
    usage
    exit 2
fi

layer=$1
case $layer in
    pipewire|x11|sway) ;;
    *)
        usage
        exit 2
        ;;
esac
shift 2
if [[ $# -eq 0 ]]; then
    usage
    exit 2
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
if ! source "$script_dir/env.sh"; then
    exit 2
fi

runtime_base="${RS_DESKTOP_RUNTIME_BASE:-/tmp}"
mkdir -p "$runtime_base"
XDG_RUNTIME_DIR="$(mktemp -d "$runtime_base/rs-desk-XXXXXX")"
chmod 0700 "$XDG_RUNTIME_DIR"
export XDG_RUNTIME_DIR

XDG_CONFIG_HOME="$XDG_RUNTIME_DIR/config"
export XDG_CONFIG_HOME
mkdir -p "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr" "$XDG_CONFIG_HOME/xdg-desktop-portal"
cp "$script_dir/sway.conf" "$XDG_CONFIG_HOME/sway.conf"
cp "$script_dir/xdpw.conf" "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr/config"
cp "$script_dir/portals.conf" "$XDG_CONFIG_HOME/xdg-desktop-portal/sway-portals.conf"

run_id="$(date -u +%Y%m%dT%H%M%S)-$$-${RANDOM}"
run_log_dir="$RS_DESKTOP_LOG_DIR/$run_id"
mkdir -p "$run_log_dir"
pgid_file="$run_log_dir/pgids"
: > "$pgid_file"

pids=()
pgids=()
process_names=()
cleanup_done=0

record_process() {
    local name=$1
    local pid=$2
    pids+=("$pid")
    pgids+=("$pid")
    process_names+=("$name")
    printf '%s %s\n' "$pid" "$name" >> "$pgid_file"
}

start_bg() {
    local name=$1
    shift
    local log_file="$run_log_dir/$name.log"
    setsid "$@" >"$log_file" 2>&1 &
    record_process "$name" "$!"
}

start_bg_fd() {
    local name=$1
    local fd_file=$2
    shift 2
    local log_file="$run_log_dir/$name.log"
    setsid "$@" 3>"$fd_file" >"$log_file" 2>&1 &
    record_process "$name" "$!"
}

group_alive() {
    local pgid=$1
    [[ $pgid =~ ^[0-9]+$ ]] || return 1
    ps -o pid= -g "$pgid" 2>/dev/null | awk 'NF { found = 1 } END { exit(found ? 0 : 1) }'
}

cleanup() {
    local status=$?
    if (( cleanup_done )); then
        return
    fi
    cleanup_done=1
    trap - EXIT INT TERM

    local i
    for ((i = ${#pgids[@]} - 1; i >= 0; i--)); do
        kill -TERM -- "-${pgids[i]}" 2>/dev/null || true
    done

    local deadline=$((SECONDS + 3))
    local any_alive=1
    while (( SECONDS < deadline )); do
        any_alive=0
        for i in "${!pgids[@]}"; do
            if group_alive "${pgids[i]}"; then
                any_alive=1
                break
            fi
        done
        (( any_alive == 0 )) && break
        sleep 0.1
    done

    for ((i = ${#pgids[@]} - 1; i >= 0; i--)); do
        if group_alive "${pgids[i]}"; then
            kill -KILL -- "-${pgids[i]}" 2>/dev/null || true
        fi
    done
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done

    local residual=0
    for i in "${!pgids[@]}"; do
        if group_alive "${pgids[i]}"; then
            printf 'process group %s (%s) still exists after cleanup\n' "${pgids[i]}" "${process_names[i]}" >&2
            residual=1
        fi
    done
    if (( residual )); then
        status=3
    fi

    rm -rf -- "$XDG_RUNTIME_DIR"
    exit "$status"
}
# Signals exit through the EXIT trap so cleanup also preserves 130/143.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

show_timeout_log() {
    local name=$1
    printf 'timeout waiting for %s after 15 seconds\n' "$name" >&2
    if [[ -f "$run_log_dir/$name.log" ]]; then
        tail -n 40 "$run_log_dir/$name.log" >&2 || true
    fi
}

wait_until() {
    local name=$1
    shift
    local attempt
    for ((attempt = 0; attempt < 150; attempt++)); do
        if "$@" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    show_timeout_log "$name"
    return 3
}

path_ready() {
    [[ -e "$1" || -S "$1" ]]
}

nonempty_file_ready() {
    [[ -s "$1" ]]
}

find_wayland_socket() {
    local socket
    for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
        if [[ -S "$socket" ]]; then
            printf '%s\n' "$socket"
            return 0
        fi
    done
    return 1
}

find_sway_socket() {
    local socket
    for socket in "$XDG_RUNTIME_DIR"/sway-ipc.*.sock; do
        if [[ -S "$socket" ]]; then
            printf '%s\n' "$socket"
            return 0
        fi
    done
    return 1
}

null_sink_ready() {
    pw-cli ls Node 2>/dev/null | awk '
        /^[[:space:]]*id[[:space:]][0-9]+,/ {
            id = $2
            sub(/,$/, "", id)
        }
        /node.name[[:space:]]*=[[:space:]]*"rs-null-sink"/ && id != "" {
            found = 1
            exit
        }
        END { exit(found ? 0 : 1) }
    '
}

null_sink_id() {
    pw-cli ls Node 2>/dev/null | awk '
        /^[[:space:]]*id[[:space:]][0-9]+,/ {
            id = $2
            sub(/,$/, "", id)
        }
        /node.name[[:space:]]*=[[:space:]]*"rs-null-sink"/ && id != "" {
            print id
            exit
        }
    '
}

dbus_name_ready() {
    local name=$1
    dbus-send --session --print-reply \
        --dest=org.freedesktop.DBus / \
        org.freedesktop.DBus.NameHasOwner "string:$name" 2>/dev/null | \
        grep -q 'boolean true'
}

portal_screencast_ready() {
    dbus-send --session --print-reply \
        --dest=org.freedesktop.portal.Desktop \
        /org/freedesktop/portal/desktop \
        org.freedesktop.DBus.Properties.Get \
        string:org.freedesktop.portal.ScreenCast string:version 2>/dev/null | \
        grep -q 'uint32'
}

start_pipewire_layer() {
    local bus_address_file="$XDG_RUNTIME_DIR/dbus-address"
    local pipewire_socket="$XDG_RUNTIME_DIR/pipewire-0"
    local null_id

    start_bg_fd dbus "$bus_address_file" \
        dbus-daemon --session --nofork --print-address=3 \
        --address="unix:path=$XDG_RUNTIME_DIR/bus"
    wait_until dbus nonempty_file_ready "$bus_address_file"
    DBUS_SESSION_BUS_ADDRESS="$(head -n 1 "$bus_address_file")"
    if [[ -z "$DBUS_SESSION_BUS_ADDRESS" ]]; then
        printf 'dbus-daemon did not provide a session address\n' >&2
        return 3
    fi
    export DBUS_SESSION_BUS_ADDRESS

    export PIPEWIRE_RUNTIME_DIR="$XDG_RUNTIME_DIR"
    start_bg pipewire pipewire
    wait_until pipewire path_ready "$pipewire_socket"
    wait_until pipewire pw-cli info 0

    start_bg wireplumber wireplumber
    pw-cli create-node adapter \
        '{ factory.name=support.null-audio-sink node.name=rs-null-sink media.class=Audio/Sink object.linger=true audio.position=[FL FR] }' \
        >/dev/null
    wait_until wireplumber null_sink_ready
    null_id="$(null_sink_id)"
    if [[ -z "$null_id" ]]; then
        printf 'could not find the rs-null-sink node id\n' >&2
        return 3
    fi
    wpctl set-default "$null_id"
    export RS_IT_NULL_SINK=rs-null-sink
}

start_x11_layer() {
    local display_file="$XDG_RUNTIME_DIR/display"
    local display_number

    unset WAYLAND_DISPLAY SWAYSOCK || true
    start_bg_fd xvfb "$display_file" \
        Xvfb -displayfd 3 -screen 0 1280x720x24 -nolisten tcp -noreset \
        -xkbdir "$XKB_CONFIG_ROOT"
    wait_until xvfb nonempty_file_ready "$display_file"
    display_number="$(tr -d '[:space:]' < "$display_file")"
    display_number="${display_number#:}"
    if [[ -z "$display_number" ]]; then
        printf 'Xvfb returned an empty display number\n' >&2
        return 3
    fi
    export DISPLAY=":$display_number"
    wait_until xvfb xset q
    unset WAYLAND_DISPLAY SWAYSOCK || true
    export XDG_SESSION_TYPE=x11
}

start_sway_layer() {
    local wayland_socket
    local sway_socket

    start_pipewire_layer
    unset DISPLAY WAYLAND_DISPLAY SWAYSOCK || true
    start_bg sway env \
        WLR_BACKENDS=headless \
        WLR_LIBINPUT_NO_DEVICES=1 \
        WLR_RENDERER=pixman \
        sway -c "$XDG_CONFIG_HOME/sway.conf"
    wait_until sway find_wayland_socket
    wayland_socket="$(find_wayland_socket)"
    export WAYLAND_DISPLAY="$(basename -- "$wayland_socket")"
    wait_until sway find_sway_socket
    sway_socket="$(find_sway_socket)"
    export SWAYSOCK="$sway_socket"
    wait_until sway swaymsg -t get_version
    unset DISPLAY || true
    export XDG_SESSION_TYPE=wayland

    start_bg portal-wlr xdg-desktop-portal-wlr
    wait_until portal-wlr dbus_name_ready \
        org.freedesktop.impl.portal.desktop.wlr

    start_bg portal xdg-desktop-portal -r
    wait_until portal portal_screencast_ready
}

case $layer in
    pipewire)
        start_pipewire_layer
        ;;
    x11)
        start_x11_layer
        ;;
    sway)
        start_sway_layer
        ;;
esac

export RS_IT_LAYER="$layer"

set +e
"$@"
command_status=$?
set -e
exit "$command_status"
