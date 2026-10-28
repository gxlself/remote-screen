#!/usr/bin/env bash
# Dry-run coverage for both netem frontends. Never changes tc, PF, or dnctl.
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
LINUX=$SCRIPT_DIR/linux.sh
MAC=$SCRIPT_DIR/mac.sh

TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/rs-netem-test.XXXXXX")
trap 'rm -rf "$TMP_DIR"' EXIT

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

run_ok() {
    local label=$1
    shift
    local output=$TMP_DIR/output
    "$@" >"$output" 2>&1 || {
        cat "$output" >&2
        fail "$label exited non-zero"
    }
    cat "$output"
}

assert_in() {
    local haystack=$1 needle=$2 label=$3
    [[ $haystack == *"$needle"* ]] || fail "$label: expected '$needle'"
}

assert_not_in() {
    local haystack=$1 needle=$2 label=$3
    [[ $haystack != *"$needle"* ]] || fail "$label: did not expect '$needle'"
}

assert_in_ci() {
    local haystack=$1 needle=$2 label=$3
    shopt -s nocasematch
    [[ $haystack == *"$needle"* ]] || {
        shopt -u nocasematch
        fail "$label: expected '$needle' (case-insensitive)"
    }
    shopt -u nocasematch
}

assert_fails() {
    local label=$1
    shift
    local output=$TMP_DIR/output
    if "$@" >"$output" 2>&1; then
        cat "$output" >&2
        fail "$label unexpectedly succeeded"
    fi
    [[ -s $output ]] || fail "$label did not print an error"
    cat "$output" >&2
}

test_linux() {
    local scenario output
    for scenario in wifi-good wifi-busy 5g-city derp weak lossy; do
        output=$(run_ok "linux $scenario" "$LINUX" --dry-run "$scenario" lo)
        assert_in "$output" 'tc qdisc replace dev lo root netem' "linux $scenario"
    done

    output=$(run_ok 'linux weak' "$LINUX" --dry-run weak lo)
    assert_in "$output" 'delay 100ms 20ms' 'linux weak delay/jitter'
    assert_in "$output" 'loss 2%' 'linux weak loss'
    assert_in "$output" 'rate 10mbit' 'linux weak rate'

    output=$(run_ok 'linux wifi-good' "$LINUX" --dry-run wifi-good lo)
    assert_in "$output" 'delay 2ms 1ms' 'linux wifi-good delay/jitter'
    assert_in "$output" 'rate 100mbit' 'linux wifi-good rate'

    output=$(run_ok 'linux 5g-city' "$LINUX" --dry-run 5g-city lo)
    assert_in "$output" 'loss 0.5%' 'linux 5g-city fractional loss'
    assert_in "$output" 'rate 40mbit' 'linux 5g-city rate'

    output=$(run_ok 'linux lossy' "$LINUX" --dry-run lossy lo)
    assert_in "$output" 'delay 30ms 10ms' 'linux lossy delay/jitter'
    assert_in "$output" 'loss 5%' 'linux lossy loss'

    output=$(run_ok 'linux udp-blocked' "$LINUX" --dry-run udp-blocked lo)
    assert_in "$output" 'udp' 'linux udp-blocked protocol'
    assert_in "$output" 'rs-netem' 'linux udp-blocked marker'
    assert_in_ci "$output" 'drop' 'linux udp-blocked verdict'
    assert_not_in "$output" 'tc qdisc replace' 'linux udp-blocked qdisc'

    output=$(run_ok 'linux handover' "$LINUX" --dry-run handover lo)
    assert_in "$output" 'loss 100%' 'linux handover down state'
    assert_in "$output" 'sleep 60' 'linux handover interval'
    assert_in "$output" 'sleep 3' 'linux handover downtime'

    output=$(run_ok 'linux bw-step' "$LINUX" --dry-run bw-step lo)
    assert_in "$output" 'rate 20mbit' 'linux bw-step initial rate'
    assert_in "$output" 'rate 3mbit' 'linux bw-step low rate'
    assert_in "$output" 'sleep 20' 'linux bw-step hold'
    [[ $(grep -c 'rate 20mbit' <<<"$output") -eq 2 ]] || fail 'linux bw-step: expected 20mbit twice'

    output=$(run_ok 'linux off' "$LINUX" --dry-run off lo)
    assert_in "$output" 'tc qdisc del dev lo root' 'linux off qdisc'
    [[ $output == *'rs-netem'* || $output == *'rs_netem'* ]] || fail 'linux off firewall: expected rs-netem cleanup'

    output=$(run_ok 'linux show' "$LINUX" --dry-run show lo)
    assert_in "$output" 'tc qdisc show dev lo' 'linux show qdisc'

    assert_fails 'linux unknown scenario' "$LINUX" --dry-run not-a-scenario lo
    assert_fails 'linux missing argument' "$LINUX" --dry-run weak
    if command -v ip >/dev/null 2>&1 && ip link show dev lo >/dev/null 2>&1; then
        if ip link show dev no-such-interface >/dev/null 2>&1; then
            fail 'test fixture: no-such-interface unexpectedly exists'
        fi
        assert_fails 'linux missing interface' "$LINUX" --dry-run weak no-such-interface
    fi
    if ((EUID != 0)); then
        output=$TMP_DIR/output
        if "$LINUX" weak lo >"$output" 2>&1; then
            fail 'linux non-root unexpectedly succeeded'
        fi
        assert_in "$(cat "$output")" 'must run as root' 'linux non-root error'
    fi
}

test_mac() {
    local scenario output
    for scenario in wifi-good wifi-busy 5g-city derp weak lossy; do
        output=$(run_ok "mac $scenario" "$MAC" --dry-run "$scenario")
        assert_in "$output" 'dnctl pipe 7400 config' "mac $scenario inbound pipe"
        assert_in "$output" 'dnctl pipe 7401 config' "mac $scenario outbound pipe"
        assert_in "$output" 'pfctl -a com.apple/rs-netem -f -' "mac $scenario anchor"
        assert_in "$output" 'pfctl -E' "mac $scenario pf enable"
        assert_in "$output" 'rs-netem.pf-token' "mac $scenario token file"
        assert_not_in "$output" 'pfctl -d' "mac $scenario must not disable PF"
    done

    output=$(run_ok 'mac weak' "$MAC" --dry-run weak)
    assert_in "$output" 'delay 100ms plr 0.02 bw 10mbit' 'mac weak delay/loss/rate'
    assert_in "$output" 'proto tcp from any to any port { 7400 } pipe 7400' 'mac weak TCP 7400'
    assert_in "$output" 'proto udp from any to any port { 7400, 41641 } pipe 7400' 'mac weak UDP ports'
    assert_in "$output" 'com.apple/rs-netem' 'mac weak anchor name'

    output=$(run_ok 'mac overridden ports' "$MAC" --dry-run --ports 7400,50000 weak)
    assert_in "$output" 'proto tcp from any to any port { 7400, 50000 }' 'mac --ports TCP'
    assert_in "$output" 'proto udp from any to any port { 7400, 50000 }' 'mac --ports UDP'

    output=$(run_ok 'mac udp-blocked' "$MAC" --dry-run udp-blocked)
    assert_in "$output" 'block drop in quick proto udp' 'mac udp-blocked inbound'
    assert_in "$output" 'block drop out quick proto udp' 'mac udp-blocked outbound'
    assert_in "$output" 'pfctl -a com.apple/rs-netem -f -' 'mac udp-blocked anchor'
    assert_not_in "$output" 'pfctl -d' 'mac udp-blocked must not disable PF'

    output=$(run_ok 'mac handover' "$MAC" --dry-run handover)
    assert_in "$output" 'plr 1' 'mac handover down state'
    assert_in "$output" 'sleep 60' 'mac handover interval'
    assert_in "$output" 'sleep 3' 'mac handover downtime'

    output=$(run_ok 'mac bw-step' "$MAC" --dry-run bw-step)
    assert_in "$output" 'bw 20mbit' 'mac bw-step initial rate'
    assert_in "$output" 'bw 3mbit' 'mac bw-step low rate'
    [[ $(grep -c 'bw 20mbit' <<<"$output") -ge 2 ]] || fail 'mac bw-step: expected 20mbit twice'

    output=$(run_ok 'mac off' "$MAC" --dry-run off)
    assert_in "$output" 'pfctl -a com.apple/rs-netem -F all' 'mac off anchor'
    assert_in "$output" 'dnctl pipe 7400 delete' 'mac off inbound pipe'
    assert_in "$output" 'dnctl pipe 7401 delete' 'mac off outbound pipe'
    assert_in "$output" '-X' 'mac off releases the PF token'
    assert_in "$output" 'rs-netem.pf-token' 'mac off token file'
    assert_not_in "$output" 'pfctl -d' 'mac off must not disable PF'

    output=$(run_ok 'mac show' "$MAC" --dry-run show)
    assert_in "$output" 'dnctl list' 'mac show dummynet'
    assert_in "$output" 'pfctl -a com.apple/rs-netem -s rules' 'mac show anchor rules'
    assert_in "$output" 'pfctl -a com.apple/rs-netem -s dummynet' 'mac show anchor dummynet rules'
    assert_not_in "$output" 'pfctl -d' 'mac show must not disable PF'

    assert_fails 'mac unknown scenario' "$MAC" --dry-run not-a-scenario
    assert_fails 'mac invalid port' "$MAC" --dry-run --ports 99999 weak
}

bash -n "$SCRIPT_DIR/common.sh" "$LINUX" "$MAC" || fail 'bash -n'
test_linux
test_mac
printf 'ALL OK\n'
