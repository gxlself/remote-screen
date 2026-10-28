#!/usr/bin/env bash
# Run the netem end-to-end scenarios repeatedly and summarize their metrics.
# Requires unshare user/net namespaces, tc, and a kernel with sch_netem.
# CI wiring is handled by another card.
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
RUNNER=$SCRIPT_DIR/netns-run.sh
# shellcheck source=tools/netem/common.sh
source "$SCRIPT_DIR/common.sh"

usage() {
    cat >&2 <<'USAGE'
Usage: tools/netem/e2e-matrix.sh [--runs N] [profile…]
USAGE
    exit 2
}

fail_usage() {
    printf 'netem matrix: %s\n' "$*" >&2
    usage
}

runs=3
profiles=()
while (($# > 0)); do
    case $1 in
        --runs)
            (($# >= 2)) || fail_usage '--runs requires a value'
            runs=$2
            shift 2
            [[ $runs =~ ^[1-9][0-9]*$ ]] || fail_usage '--runs must be a positive integer'
            ;;
        --)
            shift
            profiles+=("$@")
            break
            ;;
        --*)
            fail_usage "unknown option '$1'"
            ;;
        *)
            profiles+=("$1")
            shift
            ;;
    esac
done

if ((${#profiles[@]} == 0)); then
    profiles=(weak derp wifi-busy handover)
fi

for profile in "${profiles[@]}"; do
    profile_load "$profile" >/dev/null || exit 2
done

matrix_tmp=${TMPDIR:?TMPDIR must point to the shared temporary filesystem}/netem-matrix
mkdir -p "$matrix_tmp"

# Compile before entering a namespace because the namespace has no network.
cargo nextest run -p host-fake --test e2e --no-run

skip_marker='SKIP: unprivileged user/net namespaces unavailable'
overall_status=0
declare -a summary=()

for profile in "${profiles[@]}"; do
    for ((round = 1; round <= runs; round++)); do
        log=$matrix_tmp/$profile-$round.log
        printf '\n== %s round %d/%d ==\n' "$profile" "$round" "$runs"

        set +e
        RS_REQUIRE_NETEM=1 CARGO_NET_OFFLINE=true \
            "$RUNNER" "$profile" -- \
            cargo nextest run -p host-fake --test e2e --no-fail-fast \
                -E 'test(/netem/)' 2>&1 | tee "$log"
        pipeline_status=("${PIPESTATUS[@]}")
        set -e

        if grep -Fq "$skip_marker" "$log"; then
            printf '本机不能建 user/net namespace，矩阵未执行\n' >&2
            exit 3
        fi

        command_status=${pipeline_status[0]}
        if ((command_status == 0)); then
            result=PASS
        else
            result=FAIL
            overall_status=1
        fi

        metrics=$(sed -n \
            "s/^netem $profile: frames=\([0-9][0-9]*\) max_gap_ms=\([^ ]*\) rtt_ms=\([^ ]*\).*/\1 \2 \3/p" \
            "$log" | tail -n 1)
        if [[ -z $metrics ]]; then
            metrics='- - -'
        fi
        read -r frames max_gap_ms rtt_ms <<<"$metrics"
        summary+=("$profile|$round|$result|$frames|$max_gap_ms|$rtt_ms")
    done
done

printf '\n%-12s %-7s %-8s %-10s %-12s %-10s\n' \
    'scenario' 'round' 'result' 'frames' 'max_gap_ms' 'rtt_ms'
printf '%-12s %-7s %-8s %-10s %-12s %-10s\n' \
    '--------' '-----' '------' '------' '----------' '------'
for row in "${summary[@]}"; do
    IFS='|' read -r profile round result frames max_gap_ms rtt_ms <<<"$row"
    printf '%-12s %-7s %-8s %-10s %-12s %-10s\n' \
        "$profile" "$round" "$result" "$frames" "$max_gap_ms" "$rtt_ms"
done

exit "$overall_status"
