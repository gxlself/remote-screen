#!/usr/bin/env bash
# Run a command on isolated loopback without sudo. Each packet crosses lo's
# egress once: one-way delay is the profile delay, round-trip delay is twice it.
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
RUNNER=$SCRIPT_DIR/netns-run.sh
LINUX=$SCRIPT_DIR/linux.sh
# shellcheck source=tools/netem/common.sh
source "$SCRIPT_DIR/common.sh"

usage() {
    printf 'Usage: netns-run.sh [--dry-run] <profile|none> -- <cmd> [args…]\n' >&2
    exit 2
}

dry_run=false
inside=false
case ${1:-} in
    --dry-run) dry_run=true; shift ;;
    --inside) inside=true; shift ;;
esac
(($# >= 3)) || usage
profile=$1
shift
[[ $1 == -- ]] || usage
shift
[[ -n $1 ]] || usage
if [[ $profile != none ]]; then
    profile_load "$profile" || exit 2
fi

if [[ $dry_run == true ]]; then
    netem_quote_command unshare --user --map-root-user --net -- "$RUNNER" --inside "$profile" -- "$@"
    netem_quote_command ip link set lo up
    if [[ $profile != none ]]; then
        netem_quote_command "$LINUX" "$profile" lo --dry-run
        "$LINUX" "$profile" lo --dry-run
        if [[ $profile == handover || $profile == bw-step ]]; then
            printf '# Run linux.sh in the background; remember its PID and stop it on exit.\n'
        fi
    fi
    netem_quote_command env "RS_NETEM_PROFILE=$profile" "$@"
    if [[ $profile != none ]]; then
        printf '# On exit (also INT/TERM): stop the background profile, then clean up.\n'
        netem_quote_command "$LINUX" off lo --dry-run
    fi
    exit 0
fi

if [[ $inside == false ]]; then
    if ! unshare --user --map-root-user --net true 2>/dev/null; then
        printf 'SKIP: unprivileged user/net namespaces unavailable\n' >&2
        export RS_NETEM_PROFILE=none
        exec "$@"
    fi
    # Refuse accidental direct --inside invocation on the caller's network.
    export RS_NETEM_PARENT_NETNS
    RS_NETEM_PARENT_NETNS=$(readlink /proc/self/ns/net)
    exec unshare --user --map-root-user --net -- "$RUNNER" --inside "$profile" -- "$@"
fi

if [[ -z ${RS_NETEM_PARENT_NETNS:-} || $EUID -ne 0 ||
    $(readlink /proc/self/ns/net) == "$RS_NETEM_PARENT_NETNS" ]]; then
    printf 'netem: --inside requires a new user/net namespace created by this runner\n' >&2
    exit 2
fi
unset RS_NETEM_PARENT_NETNS
ip link set lo up

loop_pid=
command_pid=
record_pid() {
    # Shared workers keep PID records on their configured temporary filesystem.
    if [[ -n ${RS_TEST_TASK:-} && -n ${TMPDIR:-} ]]; then
        printf '%s\n' "$1" >>"$TMPDIR/$RS_TEST_TASK.pids"
    fi
}

cleanup() {
    local status=$?
    trap - EXIT INT TERM
    if [[ -n $command_pid ]]; then
        kill "$command_pid" 2>/dev/null || true
        wait "$command_pid" 2>/dev/null || true
    fi
    if [[ -n $loop_pid ]]; then
        kill "$loop_pid" 2>/dev/null || true
        wait "$loop_pid" 2>/dev/null || true
    fi
    if [[ $profile != none ]]; then
        "$LINUX" off lo || true
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ $profile != none ]]; then
    command -v tc >/dev/null 2>&1 || {
        printf 'netem: tc command is required\n' >&2
        exit 1
    }
    case $profile in
        handover | bw-step)
            "$LINUX" "$profile" lo &
            loop_pid=$!
            record_pid "$loop_pid"
            # bw-step must install its first rate before the command starts.
            if [[ $profile == bw-step ]]; then
                ready=false
                for ((attempt = 0; attempt < 100; attempt++)); do
                    kill -0 "$loop_pid" 2>/dev/null || {
                        wait "$loop_pid"
                        exit 1
                    }
                    if tc qdisc show dev lo | grep -q netem; then
                        ready=true
                        break
                    fi
                    sleep 0.05
                done
                [[ $ready == true ]] || {
                    printf 'netem: bandwidth profile did not become ready\n' >&2
                    exit 1
                }
            fi
            ;;
        *) "$LINUX" "$profile" lo ;;
    esac
fi
export RS_NETEM_PROFILE=$profile
# wait lets Bash handle signals promptly while preserving the command's status.
"$@" <&0 &
command_pid=$!
record_pid "$command_pid"
status=0
wait "$command_pid" || status=$?
command_pid=
exit "$status"
