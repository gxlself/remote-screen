#!/usr/bin/env bash

set -u

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
fuzz_dir="$repo_root/fuzz"

seconds="${1:-60}"
if ! [[ "$seconds" =~ ^[0-9]+$ ]] || (( seconds == 0 )); then
    printf 'usage: %s [positive-seconds]\n' "$0" >&2
    exit 2
fi

targets=(
    input_event
    pointer
    cursor
    clipboard_header
    tcp_record
    pair_qr
    pair_spake
    fec_recv
    wire_frames
    envelope
    trust_statement
    file_header
    file_name
    audio_datagram
    video_header
    reassembly
    pairfile
    file_ctl
    pipe_frames
    bind_node
)

if ! cargo +nightly fuzz --version >/dev/null 2>&1; then
    printf 'cargo-fuzz or the nightly toolchain is not available\n' >&2
    exit 2
fi

run_root="$(mktemp -d "${TMPDIR:-${fuzz_dir}/target}/rsp-fuzz.XXXXXX")"
trap 'rm -rf -- "$run_root"' EXIT

failed=0
declare -a summaries=()

for target in "${targets[@]}"; do
    target_file="$fuzz_dir/fuzz_targets/${target}.rs"
    if [[ ! -f "$target_file" ]]; then
        summaries+=("$target|MISSING|0|0")
        failed=1
        continue
    fi

    log="$run_root/${target}.log"
    printf '\n== %s (%ss) ==\n' "$target" "$seconds"
    (
        cd "$fuzz_dir" || exit 2
        cargo +nightly fuzz run "$target" -- -max_total_time="$seconds"
    ) 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}

    stats="$(grep -E 'DONE.*cov:.*exec/s:' "$log" | tail -n 1 || true)"
    exec_s="$(sed -nE 's/.*exec\/s: ([0-9]+).*/\1/p' <<<"$stats" | tail -n 1)"
    coverage="$(sed -nE 's/.*cov: ([0-9]+).*/\1/p' <<<"$stats" | tail -n 1)"
    exec_s="${exec_s:-0}"
    coverage="${coverage:-0}"

    if (( status != 0 )); then
        failed=1
        summaries+=("$target|CRASH($status)|$exec_s|$coverage")
    else
        summaries+=("$target|OK|$exec_s|$coverage")
    fi
done

printf '\n== summary ==\n'
printf '%-18s %-12s %10s %10s\n' target result exec/s coverage
for summary in "${summaries[@]}"; do
    IFS='|' read -r target result exec_s coverage <<<"$summary"
    printf '%-18s %-12s %10s %10s\n' "$target" "$result" "$exec_s" "$coverage"
done

exit "$failed"
