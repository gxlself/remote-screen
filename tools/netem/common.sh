#!/usr/bin/env bash
# Shared profile parsing and argument validation for the netem scripts.
# This file is sourced; it does not execute network commands.

NETEM_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
NETEM_PROFILES=${NETEM_PROFILES:-"$NETEM_ROOT/profiles.toml"}

profile_error() {
    printf 'netem: %s\n' "$*" >&2
}

# Loads the deliberately small TOML subset used by profiles.toml. It supports
# [profiles.NAME] sections, scalar key/value fields, and scalar fields inside
# an inline table or an inline array of inline tables.
profile_load() {
    local name=$1
    local output

    [[ -r $NETEM_PROFILES ]] || {
        profile_error "cannot read profile file: $NETEM_PROFILES"
        return 1
    }
    case $name in
        wifi-good | wifi-busy | 5g-city | derp | weak | lossy | udp-blocked | handover | bw-step) ;;
        *)
            profile_error "unknown scenario '$name' (known: wifi-good wifi-busy 5g-city derp weak lossy udp-blocked handover bw-step)"
            return 1
            ;;
    esac

    output=$(
        awk -v wanted="$name" '
        function strip_comment(s) {
            # Profiles contain no quoted "#"; removing from the first one is
            # sufficient for this controlled file.
            sub(/#.*/, "", s)
            return s
        }
        function trim(s) {
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", s)
            return s
        }
        function unquote(s) {
            s = trim(s)
            if (s ~ /^".*"$/) {
                s = substr(s, 2, length(s) - 2)
            }
            return s
        }
        function scalar(s, key, value,    first) {
            s = trim(s)
            if (s == "") return
            first = index(s, "=")
            if (first == 0) {
                print "netem: malformed profile entry: " s > "/dev/stderr"
                bad = 1
                return
            }
            key = trim(substr(s, 1, first - 1))
            value = unquote(trim(substr(s, first + 1)))
            if (key !~ /^[A-Za-z_][A-Za-z0-9_]*$/ || value == "") {
                print "netem: malformed profile entry: " s > "/dev/stderr"
                bad = 1
                return
            }
            print key "\t" value
        }
        function table(s, prefix,    first, body, parts, i, item, key, value, n) {
            s = trim(s)
            if (s == "") return
            first = index(s, "=")
            if (first == 0) {
                print "netem: malformed inline table: " s > "/dev/stderr"
                bad = 1
                return
            }
            body = trim(substr(s, first + 1))
            if (substr(body, 1, 1) != "{" || substr(body, length(body), 1) != "}") {
                print "netem: malformed inline table: " s > "/dev/stderr"
                bad = 1
                return
            }
            body = substr(body, 2, length(body) - 2)
            split("", parts)
            n = split(body, parts, ",")
            for (i = 1; i <= n; i++) {
                item = trim(parts[i])
                if (index(item, "=") == 0) {
                    print "netem: malformed inline table item: " item > "/dev/stderr"
                    bad = 1
                    continue
                }
                key = trim(substr(item, 1, index(item, "=") - 1))
                value = unquote(trim(substr(item, index(item, "=") + 1)))
                print prefix "." key "\t" value
            }
        }
        function array(s, prefix,    body, i) {
            s = trim(s)
            body = s
            sub(/^[^=]*=[[:space:]]*/, "", body)
            if (substr(body, 1, 1) != "[" || substr(body, length(body), 1) != "]") {
                print "netem: malformed inline array: " s > "/dev/stderr"
                bad = 1
                return
            }
            body = substr(body, 2, length(body) - 2)
            i = 1
            while (match(body, /\{[^{}]*\}/)) {
                table("x = " substr(body, RSTART, RLENGTH), prefix "." i)
                body = substr(body, RSTART + RLENGTH)
                i++
            }
            if (trim(body) != "") {
                print "netem: malformed inline array: " s > "/dev/stderr"
                bad = 1
            }
        }
        BEGIN { in_section = 0; bad = 0 }
        {
            line = strip_comment($0)
            line = trim(line)
            if (line == "") next
            if (line ~ /^\[profiles\.[A-Za-z0-9_-]+\]$/) {
                section = line
                sub(/^\[profiles\./, "", section)
                sub(/\]$/, "", section)
                in_section = (section == wanted)
                next
            }
            if (line ~ /^\[/) {
                in_section = 0
                next
            }
            if (!in_section) next
            if (line ~ /^flap[[:space:]]*=/) {
                table(line, "flap")
            } else if (line ~ /^steps[[:space:]]*=/) {
                array(line, "steps")
            } else {
                scalar(line)
            }
        }
        END { if (bad) exit 2 }
        ' "$NETEM_PROFILES"
    ) || return 1

    [[ -n $output ]] || {
        profile_error "scenario '$name' is not present in $NETEM_PROFILES"
        return 1
    }

    PROFILE_NAME=$name
    PROFILE_DELAY_MS=
    PROFILE_JITTER_MS=
    PROFILE_LOSS_PCT=
    PROFILE_RATE_MBIT=
    PROFILE_BLOCK_UDP=false
    PROFILE_FLAP_EVERY_S=
    PROFILE_FLAP_DOWN_S=
    PROFILE_STEP_RATES=()
    PROFILE_STEP_HOLDS=()
    PROFILE_STEP_COUNT=0

    local key value
    while IFS=$'\t' read -r key value; do
        case $key in
            delay_ms) PROFILE_DELAY_MS=$value ;;
            jitter_ms) PROFILE_JITTER_MS=$value ;;
            loss_pct) PROFILE_LOSS_PCT=$value ;;
            rate_mbit) PROFILE_RATE_MBIT=$value ;;
            block_udp) PROFILE_BLOCK_UDP=$value ;;
            flap.every_s) PROFILE_FLAP_EVERY_S=$value ;;
            flap.down_s) PROFILE_FLAP_DOWN_S=$value ;;
            steps.*.rate_mbit)
                PROFILE_STEP_COUNT=${key#steps.}
                PROFILE_STEP_COUNT=${PROFILE_STEP_COUNT%.rate_mbit}
                PROFILE_STEP_RATES[$((PROFILE_STEP_COUNT - 1))]=$value
                ;;
            steps.*.hold_s)
                PROFILE_STEP_COUNT=${key#steps.}
                PROFILE_STEP_COUNT=${PROFILE_STEP_COUNT%.hold_s}
                PROFILE_STEP_HOLDS[$((PROFILE_STEP_COUNT - 1))]=$value
                ;;
            *) profile_error "unknown field '$key' in scenario '$name'"; return 1 ;;
        esac
    done <<<"$output"

    # Use the highest index, not the order in which fields happened to appear.
    local count=0 index
    for index in "${!PROFILE_STEP_RATES[@]}" "${!PROFILE_STEP_HOLDS[@]}"; do
        if ((index + 1 > count)); then
            count=$((index + 1))
        fi
    done
    PROFILE_STEP_COUNT=$count
    profile_validate "$name"
}

profile_number() {
    local value=$1 label=$2 allow_off=${3:-false}
    if [[ $allow_off == true && $value == off ]]; then
        return 0
    fi
    [[ $value =~ ^[0-9]+([.][0-9]+)?$ ]] || {
        if [[ $allow_off == true ]]; then
            profile_error "scenario '$PROFILE_NAME': $label must be a non-negative number or 'off'"
        else
            profile_error "scenario '$PROFILE_NAME': $label must be a non-negative number"
        fi
        return 1
    }
}

profile_validate() {
    local name=$1
    local index rate hold
    profile_number "$PROFILE_DELAY_MS" delay_ms true || return 1
    profile_number "$PROFILE_JITTER_MS" jitter_ms true || return 1
    profile_number "$PROFILE_LOSS_PCT" loss_pct || return 1
    profile_number "$PROFILE_RATE_MBIT" rate_mbit true || return 1
    [[ $PROFILE_DELAY_MS != off || $PROFILE_JITTER_MS == off ]] || {
        profile_error "scenario '$name': jitter_ms requires delay_ms"
        return 1
    }
    [[ $PROFILE_BLOCK_UDP == true || $PROFILE_BLOCK_UDP == false ]] || {
        profile_error "scenario '$name': block_udp must be true or false"
        return 1
    }

    if [[ -n $PROFILE_FLAP_EVERY_S || -n $PROFILE_FLAP_DOWN_S ]]; then
        profile_number "$PROFILE_FLAP_EVERY_S" flap.every_s || return 1
        profile_number "$PROFILE_FLAP_DOWN_S" flap.down_s || return 1
        [[ $PROFILE_FLAP_EVERY_S != 0 && $PROFILE_FLAP_DOWN_S != 0 ]] || {
            profile_error "scenario '$name': flap intervals must be greater than zero"
            return 1
        }
    fi

    if ((PROFILE_STEP_COUNT > 0)); then
        for ((index = 0; index < PROFILE_STEP_COUNT; index++)); do
            rate=${PROFILE_STEP_RATES[index]:-}
            hold=${PROFILE_STEP_HOLDS[index]:-}
            [[ -n $rate && -n $hold ]] || {
                profile_error "scenario '$name': every step needs rate_mbit and hold_s"
                return 1
            }
            profile_number "$rate" "steps[$((index + 1))].rate_mbit" || return 1
            profile_number "$hold" "steps[$((index + 1))].hold_s" || return 1
            [[ $hold != 0 ]] || {
                profile_error "scenario '$name': step hold_s must be greater than zero"
                return 1
            }
        done
    fi
}

profile_tc_expression() {
    local rate=$1
    local expression=
    if [[ $PROFILE_DELAY_MS != off ]]; then
        expression="delay ${PROFILE_DELAY_MS}ms"
        if [[ $PROFILE_JITTER_MS != off && $PROFILE_JITTER_MS != 0 ]]; then
            expression+=" ${PROFILE_JITTER_MS}ms"
        fi
    fi
    if [[ $PROFILE_LOSS_PCT != 0 ]]; then
        expression+="${expression:+ }loss ${PROFILE_LOSS_PCT}%"
    fi
    if [[ $rate != off ]]; then
        expression+="${expression:+ }rate ${rate}mbit"
    fi
    printf '%s' "$expression"
}

netem_usage() {
    cat <<'EOF'
Usage: linux.sh <scenario|off|show> <interface> [--dry-run]
       mac.sh <scenario|off|show> [--dry-run] [--ports <ports>]

Scenarios: wifi-good wifi-busy 5g-city derp weak lossy udp-blocked handover bw-step
EOF
}

netem_parse_dry_run() {
    NETEM_DRY_RUN=false
    NETEM_POSITIONAL=()
    while (($#)); do
        case $1 in
            --dry-run) NETEM_DRY_RUN=true ;;
            --help|-h) netem_usage; return 2 ;;
            --*) profile_error "unknown option: $1"; netem_usage >&2; return 1 ;;
            *) NETEM_POSITIONAL+=("$1") ;;
        esac
        shift
    done
}

netem_quote_command() {
    local arg
    for arg in "$@"; do
        printf '%q ' "$arg"
    done
    printf '\n'
}

netem_run() {
    if [[ $NETEM_DRY_RUN == true ]]; then
        netem_quote_command "$@"
    else
        "$@"
    fi
}

# sleep that exits promptly on INT/TERM even when a loop is in its long hold.
netem_sleep() {
    local seconds=$1 remaining
    local -a sleeper
    if command -v sleep >/dev/null 2>&1; then
        sleeper=(sleep)
    else
        # POSIX sleep is still expected; this branch only gives a clearer error.
        profile_error "sleep command is required"
        return 1
    fi

    if [[ $NETEM_DRY_RUN == true ]]; then
        printf 'sleep %s\n' "$seconds"
        return
    fi

    # Split long waits so a trapped signal is handled within one second. A
    # background sleep means bash can run the trap instead of waiting for it.
    remaining=$seconds
    while ((remaining > 0)); do
        if ((remaining > 1)); then
            "${sleeper[@]}" 1 &
            wait $! || {
                local status=$?
                kill "$!" 2>/dev/null || true
                wait "$!" 2>/dev/null || true
                return "$status"
            }
            remaining=$((remaining - 1))
        else
            "${sleeper[@]}" 1
            remaining=0
        fi
    done
}
