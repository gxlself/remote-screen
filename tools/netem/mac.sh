#!/usr/bin/env bash
# Apply or remove the RS netem profiles with macOS dummynet and PF.
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tools/netem/common.sh
source "$SCRIPT_DIR/common.sh"

MAC_PIPE_IN=7400
MAC_PIPE_OUT=7401
# The stock /etc/pf.conf only references "com.apple/*" anchors, so a top-level
# "rs-netem" anchor is never evaluated. Nesting under com.apple/ is picked up by
# that wildcard without editing the system pf.conf.
MAC_PF_ANCHOR=com.apple/rs-netem
# pfctl -E hands out a reference token; it must be released with pfctl -X so we
# never disable PF that somebody else enabled.
MAC_PF_TOKEN_FILE=${TMPDIR:-/tmp}/rs-netem.pf-token
MAC_TCP_PORTS=7400
MAC_UDP_PORTS=7400,41641
MAC_LOOP_RUNNING=false

mac_usage() {
    cat <<'EOF'
Usage: mac.sh <scenario|off|show> [--dry-run] [--ports <port[,port...]>]

By default, dummynet matches TCP/UDP 7400 and WireGuard UDP 41641.
--ports replaces that port list for both TCP and UDP. It does not change the
application's actual ports.

Rules are loaded into the com.apple/rs-netem anchor, which the default
/etc/pf.conf picks up through its "com.apple/*" reference. "off" flushes that
anchor, deletes the pipes, and releases only the PF reference this script took
(recorded in ${TMPDIR:-/tmp}/rs-netem.pf-token).

Examples:
  sudo tools/netem/mac.sh weak
  sudo tools/netem/mac.sh --dry-run weak
  sudo tools/netem/mac.sh --ports 7400,41641 weak
  sudo tools/netem/mac.sh off
EOF
}

mac_parse_args() {
    NETEM_DRY_RUN=false
    NETEM_POSITIONAL=()
    while (($#)); do
        case $1 in
            --dry-run)
                NETEM_DRY_RUN=true
                shift
                ;;
            --ports)
                (($# >= 2)) || {
                    profile_error "--ports needs a value"
                    return 2
                }
                MAC_TCP_PORTS=$2
                MAC_UDP_PORTS=$2
                shift 2
                ;;
            --ports=*)
                MAC_TCP_PORTS=${1#--ports=}
                MAC_UDP_PORTS=$MAC_TCP_PORTS
                shift
                ;;
            --help|-h)
                mac_usage
                return 3
                ;;
            --*)
                profile_error "unknown option: $1"
                mac_usage >&2
                return 2
                ;;
            *)
                NETEM_POSITIONAL+=("$1")
                shift
                ;;
        esac
    done

    ((${#NETEM_POSITIONAL[@]} == 1)) || {
        mac_usage >&2
        return 2
    }
    mac_validate_ports "$MAC_TCP_PORTS" || return 2
    mac_validate_ports "$MAC_UDP_PORTS" || return 2
}

mac_validate_ports() {
    local list=$1 port
    local -a ports
    [[ -n $list ]] || {
        profile_error "--ports must not be empty"
        return 2
    }
    IFS=, read -r -a ports <<<"$list"
    ((${#ports[@]} > 0)) || {
        profile_error "--ports must contain at least one port"
        return 2
    }
    for port in "${ports[@]}"; do
        if [[ ! $port =~ ^[0-9]+$ ]] || ((port < 1 || port > 65535)); then
            profile_error "invalid port '$port' in --ports"
            return 2
        fi
    done
}

mac_require_root() {
    if [[ $NETEM_DRY_RUN != true && $EUID -ne 0 ]]; then
        profile_error "must run as root (try: sudo $0 ...)"
        exit 1
    fi
}

mac_pf_port_set() {
    local list=$1 port
    local -a ports
    local result='{ '
    IFS=, read -r -a ports <<<"$list"
    for port in "${ports[@]}"; do
        result+="$port, "
    done
    result=${result%, }
    result+=' }'
    printf '%s' "$result"
}

mac_render_anchor() {
    local tcp_ports udp_ports
    tcp_ports=$(mac_pf_port_set "$MAC_TCP_PORTS")
    udp_ports=$(mac_pf_port_set "$MAC_UDP_PORTS")

    if [[ $PROFILE_BLOCK_UDP == true ]]; then
        printf 'block drop in quick proto udp from any to any port %s\n' "$udp_ports"
        printf 'block drop out quick proto udp from any to any port %s\n' "$udp_ports"
        return
    fi

    printf 'dummynet in quick proto tcp from any to any port %s pipe %s\n' \
        "$tcp_ports" "$MAC_PIPE_IN"
    printf 'dummynet out quick proto tcp from any to any port %s pipe %s\n' \
        "$tcp_ports" "$MAC_PIPE_OUT"
    printf 'dummynet in quick proto udp from any to any port %s pipe %s\n' \
        "$udp_ports" "$MAC_PIPE_IN"
    printf 'dummynet out quick proto udp from any to any port %s pipe %s\n' \
        "$udp_ports" "$MAC_PIPE_OUT"
}

mac_load_anchor() {
    if [[ $NETEM_DRY_RUN == true ]]; then
        printf 'pfctl -a %s -f - <<%s\n' "$MAC_PF_ANCHOR" "'RS_NETEM'"
        mac_render_anchor
        printf 'RS_NETEM\n'
    else
        mac_render_anchor | pfctl -a "$MAC_PF_ANCHOR" -f -
    fi
}

mac_pf_token_read() {
    local token
    [[ -r $MAC_PF_TOKEN_FILE ]] || return 1
    token=$(<"$MAC_PF_TOKEN_FILE")
    token=${token//[[:space:]]/}
    [[ -n $token ]] || return 1
    printf '%s' "$token"
}

mac_pf_token_save() {
    if ! (umask 077 && printf '%s\n' "$1" >"$MAC_PF_TOKEN_FILE") 2>/dev/null; then
        profile_error "cannot write the PF reference token to $MAC_PF_TOKEN_FILE"
        return 1
    fi
}

mac_release_pf_token() {
    local token
    if [[ $NETEM_DRY_RUN == true ]]; then
        printf 'pfctl -X "$(cat %q)"\n' "$MAC_PF_TOKEN_FILE"
        return
    fi
    if ! token=$(mac_pf_token_read); then
        # No reference held by us; "off" stays repeatable.
        return 0
    fi
    if pfctl -X "$token" >/dev/null 2>&1; then
        printf 'netem: released PF reference token %s\n' "$token" >&2
    else
        # Keep the token file so a later "off" can retry instead of leaking the
        # reference silently.
        printf 'netem: warning: pfctl -X %s failed; PF may stay enabled (token kept in %s)\n' \
            "$token" "$MAC_PF_TOKEN_FILE" >&2
        return 0
    fi
    rm -f -- "$MAC_PF_TOKEN_FILE"
}

mac_config_pipe() {
    local pipe=$1 rate=$2 loss_pct=$3
    local -a command=(dnctl pipe "$pipe" config)
    if [[ $PROFILE_DELAY_MS != off ]]; then
        command+=(delay "${PROFILE_DELAY_MS}ms")
        # dnctl's delay syntax is "delay <ms>"; dummynet has no jitter option.
        if [[ $PROFILE_JITTER_MS != off && $PROFILE_JITTER_MS != 0 ]]; then
            printf 'netem: note: dummynet has no jitter support; ignoring jitter_ms=%s for %s\n' \
                "$PROFILE_JITTER_MS" "$PROFILE_NAME" >&2
        fi
    fi
    command+=(plr "$(profile_loss_fraction_for "$loss_pct")")
    # A zero rate keeps the pipe unlimited. Passing "bw 0mbit" is not valid
    # on every supported macOS version.
    if [[ $rate != off && $rate != 0 && $rate != 0.0 ]]; then
        command+=(bw "${rate}mbit")
    fi
    netem_run "${command[@]}"
}

profile_loss_fraction_for() {
    awk -v pct="$1" 'BEGIN {
        value = pct / 100
        text = sprintf("%.6f", value)
        sub(/0+$/, "", text)
        sub(/[.]$/, "", text)
        print text
    }'
}

mac_config_both_pipes() {
    local rate=$1 loss_pct=$2
    mac_config_pipe "$MAC_PIPE_IN" "$rate" "$loss_pct"
    mac_config_pipe "$MAC_PIPE_OUT" "$rate" "$loss_pct"
}

mac_try() {
    if [[ $NETEM_DRY_RUN == true ]]; then
        netem_quote_command "$@"
    else
        "$@" >/dev/null 2>&1 || true
    fi
}

mac_reset() {
    if [[ $NETEM_DRY_RUN == true ]]; then
        netem_quote_command pfctl -a "$MAC_PF_ANCHOR" -F all
    else
        pfctl -a "$MAC_PF_ANCHOR" -F all >/dev/null 2>&1 || true
    fi
    mac_try dnctl pipe "$MAC_PIPE_IN" delete
    mac_try dnctl pipe "$MAC_PIPE_OUT" delete
    mac_release_pf_token
}

mac_enable_pf() {
    if [[ $NETEM_DRY_RUN == true ]]; then
        netem_quote_command pfctl -E
        printf 'netem: would record the PF reference token in %s\n' "$MAC_PF_TOKEN_FILE"
        return
    fi

    local output token
    # Release a reference left over from an interrupted run before taking a new
    # one, so a stale token file cannot leak a PF reference.
    mac_release_pf_token
    if [[ -e $MAC_PF_TOKEN_FILE ]]; then
        profile_error "stale PF reference token in $MAC_PF_TOKEN_FILE could not be released; run 'off' again"
        return 1
    fi
    if ! output=$(pfctl -E 2>&1); then
        profile_error "pfctl -E failed: $output (run 'off' to clear the anchor and pipes)"
        return 1
    fi
    # "pfctl -E" prints a line like "Token : 1234567890"; keep only the digits.
    token=$(printf '%s\n' "$output" | awk '
        /[Tt]oken/ {
            line = $0
            sub(/^[^:]*:/, "", line)
            gsub(/[^0-9]/, "", line)
            if (line != "") {
                print line
                exit
            }
        }')
    if [[ -z $token ]]; then
        profile_error "could not parse the PF reference token from pfctl -E output"
        return 1
    fi
    if ! mac_pf_token_save "$token"; then
        # Do not leak the reference we just took.
        pfctl -X "$token" >/dev/null 2>&1 || true
        return 1
    fi
    printf 'netem: PF enabled (reference token %s)\n' "$token" >&2
}

mac_stop_loop() {
    local status=$1
    trap - EXIT INT TERM
    if [[ $MAC_LOOP_RUNNING == true ]]; then
        printf 'netem: restoring dummynet and PF on macOS\n' >&2
        mac_reset
    fi
    exit "$status"
}

mac_begin_loop() {
    MAC_LOOP_RUNNING=true
    trap 'mac_stop_loop $?' EXIT
    trap 'mac_stop_loop 130' INT
    trap 'mac_stop_loop 143' TERM
}

mac_run_handover() {
    local interval=$PROFILE_FLAP_EVERY_S
    local downtime=$PROFILE_FLAP_DOWN_S

    if [[ $NETEM_DRY_RUN == true ]]; then
        mac_reset
        mac_config_both_pipes 0 0
        mac_load_anchor
        mac_enable_pf
        netem_sleep "$interval"
        mac_config_both_pipes 0 100
        netem_sleep "$downtime"
        mac_config_both_pipes 0 0
        return
    fi

    mac_reset
    mac_config_both_pipes 0 0
    mac_load_anchor
    mac_enable_pf
    mac_begin_loop
    printf 'netem: handover: up %ss / down %ss; Ctrl-C to restore\n' \
        "$interval" "$downtime" >&2
    while true; do
        netem_sleep "$interval"
        mac_config_pipe "$MAC_PIPE_IN" 0 100
        mac_config_pipe "$MAC_PIPE_OUT" 0 100
        netem_sleep "$downtime"
        mac_config_pipe "$MAC_PIPE_IN" 0 0
        mac_config_pipe "$MAC_PIPE_OUT" 0 0
    done
}

mac_run_steps() {
    local index rate hold

    if [[ $NETEM_DRY_RUN == true ]]; then
        mac_reset
        mac_load_anchor
        mac_enable_pf
        for ((index = 0; index < PROFILE_STEP_COUNT; index++)); do
            rate=${PROFILE_STEP_RATES[index]}
            hold=${PROFILE_STEP_HOLDS[index]}
            mac_config_both_pipes "$rate" "$PROFILE_LOSS_PCT"
            netem_sleep "$hold"
        done
        return
    fi

    mac_reset
    mac_load_anchor
    mac_enable_pf
    mac_begin_loop
    printf 'netem: bandwidth steps; Ctrl-C to restore\n' >&2
    while true; do
        for ((index = 0; index < PROFILE_STEP_COUNT; index++)); do
            rate=${PROFILE_STEP_RATES[index]}
            hold=${PROFILE_STEP_HOLDS[index]}
            mac_config_both_pipes "$rate" "$PROFILE_LOSS_PCT"
            printf 'netem: rate %smbit for %ss\n' "$rate" "$hold" >&2
            netem_sleep "$hold"
        done
    done
}

mac_run_static() {
    if [[ $PROFILE_BLOCK_UDP == true ]]; then
        mac_reset
        mac_load_anchor
        mac_enable_pf
    else
        mac_reset
        mac_config_both_pipes "$PROFILE_RATE_MBIT" "$PROFILE_LOSS_PCT"
        mac_load_anchor
        mac_enable_pf
    fi
}

mac_show() {
    printf 'Anchor: %s\n' "$MAC_PF_ANCHOR"
    printf 'Pipes: %s (in), %s (out)\n' "$MAC_PIPE_IN" "$MAC_PIPE_OUT"
    printf 'Ports: TCP %s; UDP %s\n' "$MAC_TCP_PORTS" "$MAC_UDP_PORTS"
    if [[ $NETEM_DRY_RUN != true ]] && [[ -r $MAC_PF_TOKEN_FILE ]]; then
        printf 'PF reference token file: %s\n' "$MAC_PF_TOKEN_FILE"
    fi
    netem_run dnctl list
    netem_run pfctl -a "$MAC_PF_ANCHOR" -s rules
    netem_run pfctl -a "$MAC_PF_ANCHOR" -s dummynet
}

main() {
    mac_parse_args "$@" || {
        local status=$?
        [[ $status -eq 3 ]] && exit 0
        exit "$status"
    }

    local action=${NETEM_POSITIONAL[0]}
    case $action in
        off)
            mac_require_root
            mac_reset
            ;;
        show)
            mac_require_root
            mac_show
            ;;
        *)
            if ! profile_load "$action"; then
                exit 2
            fi
            mac_require_root
            if [[ -n $PROFILE_FLAP_EVERY_S ]]; then
                mac_run_handover
            elif ((PROFILE_STEP_COUNT > 0)); then
                mac_run_steps
            elif [[ $PROFILE_BLOCK_UDP == true || $PROFILE_RATE_MBIT != off ]]; then
                mac_run_static
            else
                profile_error "scenario '$action' has no applicable settings"
                exit 2
            fi
            ;;
    esac
}

main "$@"
