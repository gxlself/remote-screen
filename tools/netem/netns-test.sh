#!/usr/bin/env bash
# Unprivileged dry-run checks, plus a real smoke test when namespaces work.
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
RUNNER=$SCRIPT_DIR/netns-run.sh

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

assert_in() {
    local haystack=$1 needle=$2 label=$3
    [[ $haystack == *"$needle"* ]] || fail "$label: expected '$needle'"
}

assert_status() {
    local expected=$1 status=0
    shift
    "$@" >/dev/null 2>&1 || status=$?
    [[ $status -eq $expected ]] || fail "expected status $expected, got $status: $*"
}

# Bash has a TCP client but no listener. Python supplies only an ephemeral
# loopback listener; the connection being timed is made by bash /dev/tcp.
tcp_smoke() {
    command -v python3 >/dev/null 2>&1 || fail 'TCP smoke without ping needs python3'
    "$RUNNER" weak -- bash -c 'python3 -' <<'PY'
import socket
import subprocess
import threading
import time

with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    listener.settimeout(10)
    def accept():
        connection, _ = listener.accept()
        connection.close()
    worker = threading.Thread(target=accept, daemon=True)
    worker.start()
    started = time.monotonic()
    subprocess.run(
        ["bash", "-c", 'exec 3<>/dev/tcp/127.0.0.1/"$1"',
         "tcp-smoke", str(listener.getsockname()[1])],
        check=True, timeout=10,
    )
    print(f"{(time.monotonic() - started) * 1000:.3f}")
    worker.join(timeout=10)
PY
}

bash -n "$RUNNER" "$SCRIPT_DIR/netns-test.sh" || fail 'bash -n'
for profile in none weak udp-blocked handover bw-step wifi-good wifi-busy 5g-city derp lossy; do
    output=$("$RUNNER" --dry-run "$profile" -- bash -c 'exit 99') || fail "$profile dry run"
    assert_in "$output" 'unshare --user --map-root-user --net' "$profile namespace"
    assert_in "$output" "RS_NETEM_PROFILE=$profile" "$profile environment"
    if [[ $profile != none ]]; then
        assert_in "$output" 'linux.sh' "$profile frontend"
        assert_in "$output" "${profile} lo --dry-run" "$profile loopback"
    fi
done
assert_status 2 "$RUNNER" --dry-run unknown-profile -- true
assert_status 2 "$RUNNER" --dry-run weak true
assert_status 2 "$RUNNER" --dry-run weak --
assert_status 2 "$RUNNER" --inside none -- true

if unshare --user --map-root-user --net true 2>/dev/null; then
    if command -v ping >/dev/null 2>&1; then
        output=$(LC_ALL=C "$RUNNER" weak -- sh -c 'ping -c 3 -i 0.2 127.0.0.1') || fail 'weak ping smoke'
        printf '%s\n' "$output"
        average=$(awk -F '= ' '/^(rtt|round-trip)/ {split($2, times, "/"); print times[2]}' <<<"$output")
    else
        average=$(tcp_smoke) || fail 'weak TCP smoke'
    fi
    [[ -n $average ]] || fail 'weak smoke did not report RTT'
    awk -v ms="$average" 'BEGIN {exit !(ms >= 180)}' || fail "weak RTT $average ms < 180 ms"
    printf 'weak average RTT: %s ms\n' "$average"
    output=$("$RUNNER" none -- sh -c 'echo $RS_NETEM_PROFILE') || fail 'none smoke'
    [[ $output == none ]] || fail "none smoke: got '$output'"
else
    printf 'real smoke skipped: unprivileged user/net namespaces unavailable\n'
    # A denied namespace must still run the command, preserving argv, stdin,
    # and exit status while replacing any inherited/requested profile by none.
    output=$(RS_NETEM_PROFILE=weak "$RUNNER" weak -- sh -c '
        read -r input
        test "$1" = "argument with spaces" && test "$input" = "stdin survives" || exit 1
        printf "%s\n" "$RS_NETEM_PROFILE"
    ' _ 'argument with spaces' <<< 'stdin survives') || fail 'fallback command'
    [[ $output == none ]] || fail "fallback profile: got '$output'"
    assert_status 37 "$RUNNER" weak -- sh -c 'exit 37'
fi
printf 'ALL OK\n'
