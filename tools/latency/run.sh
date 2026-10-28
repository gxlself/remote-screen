#!/usr/bin/env bash
# Run one host-fake latency sample. Listeners stay on loopback and inside the
# task's allocated RS_TEST_PORT_BASE..+SPAN range.
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$ROOT"

SECS=${1:-60}
OUT=${2:-target/latency/latest.json}
PORT_BASE=${RS_TEST_PORT_BASE:-33400}
PORT_SPAN=${RS_TEST_PORT_SPAN:-20}
PIDS=${RS_TEST_TASK:+${TMPDIR:-/tmp}/$RS_TEST_TASK.pids}
PIDS=${PIDS:-${TMPDIR:-/tmp}/rs-latency-run.pids}
LOG_DIR=${TMPDIR:-/tmp}/rs-latency-run
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}

if ! [[ $SECS =~ ^[1-9][0-9]*$ ]]; then
    echo "latency: SECS must be a positive integer, got '$SECS'" >&2
    exit 2
fi
if ! [[ $PORT_BASE =~ ^[0-9]+$ && $PORT_SPAN =~ ^[0-9]+$ ]]; then
    echo "latency: RS_TEST_PORT_BASE/SPAN must be numeric" >&2
    exit 2
fi

mkdir -p "$(dirname -- "$OUT")" "$LOG_DIR" "$(dirname -- "$PIDS")"
if [[ $TARGET_DIR != /* ]]; then
    TARGET_DIR=$ROOT/$TARGET_DIR
fi
touch "$PIDS"

# A private network namespace keeps tc netem off the shared development host.
if [[ ${RS_LATENCY_NETNS:-0} != 1 ]]; then
    if TMPDIR=${TMPDIR:-/tmp} unshare --user --map-root-user --net true >/dev/null 2>&1; then
        RS_LATENCY_NETNS=1 TMPDIR=${TMPDIR:-/tmp} exec unshare --user --map-root-user --net -- \
            bash "$0" "$SECS" "$OUT"
    elif TMPDIR=${TMPDIR:-/tmp} unshare --net true >/dev/null 2>&1; then
        RS_LATENCY_NETNS=1 TMPDIR=${TMPDIR:-/tmp} exec unshare --net -- bash "$0" "$SECS" "$OUT"
    fi
fi

HOST_PID=
PROXY_PID=
SHAPING=none
NETEM_ACTIVE=false
declare -a STARTED=()

record_pid() {
    local pid=$1
    STARTED+=("$pid")
    printf '%s\n' "$pid" >>"$PIDS"
}

stop_pid() {
    local pid=$1
    [[ -n $pid ]] || return 0
    if kill -0 "$pid" 2>/dev/null; then
        kill -TERM "$pid" 2>/dev/null || true
        for _ in {1..50}; do
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.1
        done
        if kill -0 "$pid" 2>/dev/null; then
            kill -KILL "$pid" 2>/dev/null || true
        fi
    fi
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    local status=$?
    trap - EXIT INT TERM
    if [[ $NETEM_ACTIVE == true ]]; then
        "$ROOT/tools/netem/linux.sh" off lo >/dev/null 2>&1 || true
    fi
    stop_pid "$PROXY_PID"
    stop_pid "$HOST_PID"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

free_ports() {
    python3 - "$1" "$2" <<'PY'
import socket
import sys

base, span = map(int, sys.argv[1:])
free = []
for port in range(base, base + span):
    usable = True
    for kind in (socket.SOCK_DGRAM, socket.SOCK_STREAM):
        sock = socket.socket(socket.AF_INET, kind)
        try:
            sock.bind(("127.0.0.1", port))
        except OSError:
            usable = False
        finally:
            sock.close()
    if usable:
        free.append(port)
    if len(free) == 2:
        print(f"{free[0]} {free[1]}")
        break
else:
    raise SystemExit(f"need two free ports in {base}-{base + span - 1}")
PY
}

read -r HOST_PORT PROXY_PORT < <(free_ports "$PORT_BASE" "$PORT_SPAN")
IDENTITY_DIR=target/latency/identity
HOST_STATE=$LOG_DIR/host-state
mkdir -p "$HOST_STATE"

if [[ ${RS_LATENCY_NETNS:-0} == 1 ]]; then
    ip link set lo up
fi

echo 'latency: building host-fake, probe and faultnet launcher' >&2
cargo build -q -p rs-host-agent --features fake 2>&1 | tee "$LOG_DIR/build.log"
cargo build -q -p rs-probe 2>&1 | tee -a "$LOG_DIR/build.log"
cargo build -q -p rs-latency-faultnet 2>&1 | tee -a "$LOG_DIR/build.log"

rm -f -- "$OUT"

HOST_LOG=$LOG_DIR/host.log
PROXY_LOG=$LOG_DIR/proxy.log
PROBE_LOG=$LOG_DIR/probe.log

"$TARGET_DIR/debug/rs-host" --platform fake --bind "127.0.0.1:$HOST_PORT" \
    --dev-allow-unpaired --state-dir "$HOST_STATE" >"$HOST_LOG" 2>&1 &
HOST_PID=$!
record_pid "$HOST_PID"

ready=false
for _ in {1..150}; do
    if ! kill -0 "$HOST_PID" 2>/dev/null; then
        break
    fi
    if (exec 3<>"/dev/tcp/127.0.0.1/$HOST_PORT") 2>/dev/null; then
        ready=true
        break
    fi
    sleep 0.1
done
if [[ $ready != true ]]; then
    echo "latency: host-fake did not become ready on 127.0.0.1:$HOST_PORT" >&2
    tail -n 40 "$HOST_LOG" >&2 || true
    exit 1
fi

PROBE_ADDR="127.0.0.1:$HOST_PORT"
if [[ ${RS_LATENCY_NETNS:-0} == 1 ]]; then
    if "$ROOT/tools/netem/linux.sh" wifi-good lo >/dev/null 2>&1; then
        SHAPING=netem-wifi-good
        NETEM_ACTIVE=true
    fi
fi

if [[ $SHAPING == none ]]; then
    : >"$PROXY_LOG"
    "$TARGET_DIR/debug/rs-latency-faultnet" \
        --upstream "127.0.0.1:$HOST_PORT" --listen "127.0.0.1:$PROXY_PORT" \
        >"$PROXY_LOG" 2>&1 &
    PROXY_PID=$!
    record_pid "$PROXY_PID"
    proxy_ready=false
    for _ in {1..100}; do
        if grep -q '"addr"' "$PROXY_LOG" 2>/dev/null; then
            proxy_ready=true
            break
        fi
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    if [[ $proxy_ready == true ]]; then
        PROBE_ADDR="127.0.0.1:$PROXY_PORT"
        SHAPING=faultnet-delay-2ms-jitter-1ms-100mbit
    else
        echo 'latency: faultnet proxy unavailable; continuing with shaping=none' >&2
        tail -n 40 "$PROXY_LOG" >&2 || true
        stop_pid "$PROXY_PID"
        PROXY_PID=
    fi
fi

RS_GIT_REV=$(git rev-parse --verify HEAD 2>/dev/null || true) \
    "$TARGET_DIR/debug/rs-probe" --addr "$PROBE_ADDR" --seconds "$SECS" \
    --json "$OUT" --identity "$IDENTITY_DIR" --dev-allow-unpaired >"$PROBE_LOG" 2>&1

tail -n 40 "$PROBE_LOG"
python3 - "$OUT" "$SHAPING" <<'PY'
import json
import sys
import time
from pathlib import Path

path = Path(sys.argv[1])
shaping = sys.argv[2]
report = json.loads(path.read_text(encoding="utf-8"))
report["shaping"] = shaping
report["timestamp_us"] = time.time_ns() // 1_000
path.write_text(json.dumps(report, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
PY

cat "$OUT"
