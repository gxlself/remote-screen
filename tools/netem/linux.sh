#!/usr/bin/env bash
# Apply or remove the RS netem profiles on a Linux host / soft router.
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tools/netem/common.sh
source "$SCRIPT_DIR/common.sh"

NETEM_LOOP_RUNNING=false
NETEM_IFACE=

linux_usage() {
    cat <<'EOF'
Usage: linux.sh <scenario|off|show> <interface> [--dry-run]

Examples:
  sudo tools/netem/linux.sh weak wlan0
  sudo tools/netem/linux.sh handover wlan0
  sudo tools/netem/linux.sh --dry-run weak wlan0
  sudo tools/netem/linux.sh off wlan0
EOF
}

linux_require_root() {
    if [[ $NETEM_DRY_RUN != true && $EUID -ne 0 ]]; then
        profile_error "must run as root (try: sudo $0 ...)"
        exit 1
    fi
}

linux_validate_interface() {
    NETEM_IFACE=$1
    if [[ ! $NETEM_IFACE =~ ^[A-Za-z0-9_.:@-]+$ ]]; then
        profile_error "invalid interface name: $NETEM_IFACE"
        exit 2
    fi
    if ! command -v ip >/dev/null 2>&1; then
        if [[ $NETEM_DRY_RUN == true ]]; then
            # Keep dry runs usable on build machines without iproute2.
            return
        fi
        profile_error "ip command is required to validate the interface"
        exit 1
    fi
    if ! ip link show dev "$NETEM_IFACE" >/dev/null 2>&1; then
        profile_error "interface does not exist: $NETEM_IFACE"
        exit 2
    fi
}

linux_netem_command() {
    local rate=$1
    local expression
    local -a expression_args
    expression=$(profile_tc_expression "$rate")
    if [[ -z $expression ]]; then
        profile_error "scenario '$PROFILE_NAME' has no netem parameters"
        return 1
    fi
    read -r -a expression_args <<<"$expression"
    netem_run tc qdisc replace dev "$NETEM_IFACE" root netem "${expression_args[@]}"
}

linux_try() {
    if [[ $NETEM_DRY_RUN == true ]]; then
        netem_quote_command "$@"
    else
        "$@" >/dev/null 2>&1 || true
    fi
}

linux_clear_qdisc() {
    linux_try tc qdisc del dev "$NETEM_IFACE" root
}

linux_firewall_backend() {
    if command -v nft >/dev/null 2>&1 && nft list ruleset >/dev/null 2>&1; then
        printf 'nft\n'
    elif command -v iptables >/dev/null 2>&1 && iptables -w -S INPUT >/dev/null 2>&1; then
        printf 'iptables\n'
    elif [[ $NETEM_DRY_RUN == true ]]; then
        # A dry run must be usable on a workstation without firewall tools.
        printf 'iptables\n'
    else
        return 1
    fi
}

linux_clear_iptables_rules() {
    local chain rule
    local -a rule_args
    for chain in INPUT OUTPUT FORWARD; do
        while IFS= read -r rule; do
            [[ $rule == *'--comment'* && $rule == *'rs-netem'* ]] || continue
            rule=${rule#-A "$chain" }
            # Some iptables versions quote comments in -S output. Our comment
            # and validated interface name contain no quotes, so removing
            # them restores the original argv without eval.
            rule=${rule//\"/}
            read -r -a rule_args <<<"$rule"
            linux_try iptables -w -D "$chain" "${rule_args[@]}"
        done < <(iptables -w -S "$chain" 2>/dev/null || true)
    done
}

linux_clear_firewall() {
    local installed=false
    if command -v nft >/dev/null 2>&1; then
        installed=true
        if [[ $NETEM_DRY_RUN == true ]]; then
            netem_quote_command nft delete table inet rs_netem
        else
            nft delete table inet rs_netem >/dev/null 2>&1 || true
        fi
    fi
    if command -v iptables >/dev/null 2>&1; then
        installed=true
        if [[ $NETEM_DRY_RUN == true ]]; then
            local chain
            for chain in INPUT OUTPUT FORWARD; do
                netem_quote_command iptables -w -D "$chain" -m comment --comment rs-netem -j DROP
            done
        else
            linux_clear_iptables_rules
        fi
    fi
    if [[ $installed == false && $NETEM_DRY_RUN == true ]]; then
        # Show a representative cleanup command on build machines that have
        # neither firewall frontend installed.
        netem_quote_command iptables -w -D INPUT -m comment --comment rs-netem -j DROP
    fi
}

linux_install_firewall() {
    local backend
    backend=$(linux_firewall_backend) || {
        profile_error "udp-blocked needs nft or iptables"
        exit 1
    }
    if [[ $backend == nft ]]; then
        netem_run nft add table inet rs_netem
        netem_run nft add chain inet rs_netem input '{ type filter hook input priority 0; policy accept; }'
        netem_run nft add chain inet rs_netem output '{ type filter hook output priority 0; policy accept; }'
        netem_run nft add chain inet rs_netem forward '{ type filter hook forward priority 0; policy accept; }'
        netem_run nft add rule inet rs_netem input iifname "$NETEM_IFACE" udp counter comment rs-netem drop
        netem_run nft add rule inet rs_netem output oifname "$NETEM_IFACE" udp counter comment rs-netem drop
        netem_run nft add rule inet rs_netem forward iifname "$NETEM_IFACE" udp counter comment rs-netem drop
        netem_run nft add rule inet rs_netem forward oifname "$NETEM_IFACE" udp counter comment rs-netem drop
    else
        netem_run iptables -w -I INPUT -i "$NETEM_IFACE" -p udp -m comment --comment rs-netem -j DROP
        netem_run iptables -w -I OUTPUT -o "$NETEM_IFACE" -p udp -m comment --comment rs-netem -j DROP
        netem_run iptables -w -I FORWARD -i "$NETEM_IFACE" -p udp -m comment --comment rs-netem -j DROP
        netem_run iptables -w -I FORWARD -o "$NETEM_IFACE" -p udp -m comment --comment rs-netem -j DROP
    fi
}

linux_reset_quiet() {
    linux_clear_qdisc
    linux_clear_firewall
}

linux_stop_loop() {
    local status=$1
    trap - EXIT INT TERM
    if [[ $NETEM_LOOP_RUNNING == true ]]; then
        printf 'netem: restoring network on %s\n' "$NETEM_IFACE" >&2
        linux_reset_quiet
    fi
    exit "$status"
}

linux_run_handover() {
    local interval=$PROFILE_FLAP_EVERY_S
    local downtime=$PROFILE_FLAP_DOWN_S
    local -a down_args=(loss 100%)

    if [[ $NETEM_DRY_RUN == true ]]; then
        netem_quote_command tc qdisc del dev "$NETEM_IFACE" root
        netem_quote_command tc qdisc replace dev "$NETEM_IFACE" root netem "${down_args[@]}"
        netem_sleep "$interval"
        netem_quote_command tc qdisc del dev "$NETEM_IFACE" root
        netem_sleep "$downtime"
        return
    fi

    NETEM_LOOP_RUNNING=true
    trap 'linux_stop_loop $?' EXIT
    trap 'linux_stop_loop 130' INT
    trap 'linux_stop_loop 143' TERM
    printf 'netem: handover on %s: up %ss / down %ss; Ctrl-C to restore\n' \
        "$NETEM_IFACE" "$interval" "$downtime" >&2
    while true; do
        netem_sleep "$interval"
        tc qdisc replace dev "$NETEM_IFACE" root netem "${down_args[@]}"
        netem_sleep "$downtime"
        tc qdisc del dev "$NETEM_IFACE" root
    done
}

linux_run_steps() {
    local index rate hold
    local -a expression_args
    local expression

    if [[ $NETEM_DRY_RUN == true ]]; then
        for ((index = 0; index < PROFILE_STEP_COUNT; index++)); do
            rate=${PROFILE_STEP_RATES[index]}
            hold=${PROFILE_STEP_HOLDS[index]}
            expression=$(profile_tc_expression "$rate")
            read -r -a expression_args <<<"$expression"
            netem_quote_command tc qdisc replace dev "$NETEM_IFACE" root netem "${expression_args[@]}"
            netem_sleep "$hold"
        done
        return
    fi

    NETEM_LOOP_RUNNING=true
    trap 'linux_stop_loop $?' EXIT
    trap 'linux_stop_loop 130' INT
    trap 'linux_stop_loop 143' TERM
    printf 'netem: bandwidth steps on %s; Ctrl-C to restore\n' "$NETEM_IFACE" >&2
    while true; do
        for ((index = 0; index < PROFILE_STEP_COUNT; index++)); do
            rate=${PROFILE_STEP_RATES[index]}
            hold=${PROFILE_STEP_HOLDS[index]}
            expression=$(profile_tc_expression "$rate")
            read -r -a expression_args <<<"$expression"
            tc qdisc replace dev "$NETEM_IFACE" root netem "${expression_args[@]}"
            printf 'netem: rate %smbit for %ss\n' "$rate" "$hold" >&2
            netem_sleep "$hold"
        done
    done
}

linux_show() {
    printf 'Interface: %s\n' "$NETEM_IFACE"
    printf 'Profiles: %s\n' "$NETEM_PROFILES"
    netem_run tc qdisc show dev "$NETEM_IFACE"

    if [[ $NETEM_DRY_RUN == true ]]; then
        netem_quote_command nft list table inet rs_netem
        netem_quote_command iptables -w -S INPUT
        netem_quote_command iptables -w -S OUTPUT
        netem_quote_command iptables -w -S FORWARD
        return
    fi

    if command -v nft >/dev/null 2>&1; then
        if ! nft list table inet rs_netem 2>/dev/null; then
            printf 'rs-netem nft table: absent\n'
        fi
    fi
    if command -v iptables >/dev/null 2>&1; then
        local chain
        for chain in INPUT OUTPUT FORWARD; do
            iptables -w -S "$chain" 2>/dev/null | awk -v c="$chain" '$0 ~ /--comment rs-netem/ { print c ": " $0 }'
        done
    fi
}

main() {
    netem_parse_dry_run "$@" || {
        local status=$?
        [[ $status -eq 2 ]] && exit 0
        exit "$status"
    }
    if ((${#NETEM_POSITIONAL[@]} != 2)); then
        linux_usage >&2
        exit 2
    fi

    local action=${NETEM_POSITIONAL[0]}
    linux_require_root
    linux_validate_interface "${NETEM_POSITIONAL[1]}"

    case $action in
        show)
            linux_show
            ;;
        off)
            linux_reset_quiet
            ;;
        *)
            if ! profile_load "$action"; then
                # "off"/"show" are actions; any other first argument is a
                # profile name. Surface this as a usage error (2), while
                # profile parsing itself also fails closed.
                exit 2
            fi
            linux_reset_quiet
            if [[ $PROFILE_BLOCK_UDP == true ]]; then
                linux_install_firewall
            elif [[ -n $PROFILE_FLAP_EVERY_S ]]; then
                linux_run_handover
            elif ((PROFILE_STEP_COUNT > 0)); then
                linux_run_steps
            elif [[ $PROFILE_RATE_MBIT != off ]]; then
                linux_netem_command "$PROFILE_RATE_MBIT"
            else
                profile_error "scenario '$action' has no applicable settings"
                exit 2
            fi
            ;;
    esac
}

main "$@"
