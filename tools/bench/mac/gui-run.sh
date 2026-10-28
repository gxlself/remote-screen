#!/bin/bash
# Run the target executable directly in this user's logged-in Aqua session.
# Do not put a shell in ProgramArguments: it would become TCC's responsible process.
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
usage: gui-run.sh [--timeout <seconds>] [--label <label>] [--cwd <directory>] [--detach] [--dry-run] -- <command> [args...]
       gui-run.sh --wait <label> [--timeout <seconds>]
       gui-run.sh --stop <label>
       gui-run.sh --parse-status
USAGE
    if [ "$#" -gt 0 ]; then printf 'gui-run: %s\n' "$1" >&2; fi
    exit 2
}

# Keep the polling path and --parse-status on exactly the same parser. In
# particular, a not-yet-started job's "(never exited)" is not a successful exit.
parse_status() {
    awk '
        /^[[:space:]]*state[[:space:]]*=[[:space:]]*running[[:space:]]*$/ { running = 1 }
        /^[[:space:]]*last exit code[[:space:]]*=[[:space:]]*[0-9]+[[:space:]]*$/ {
            code = $0
            sub(/^.*=[[:space:]]*/, "", code)
            sub(/[[:space:]]*$/, "", code)
            have_code = 1
        }
        /^[[:space:]]*last terminating signal[[:space:]]*=[[:space:]]*.*[0-9]+[[:space:]]*$/ {
            signal = $0
            sub(/[[:space:]]*$/, "", signal)
            sub(/^.*[^0-9]/, "", signal)
            have_signal = 1
        }
        END {
            if (running) print "running"
            else if (have_signal) print "signal " signal
            else if (have_code) print "exit " code
            else print "unknown"
        }
    '
}

validate_label() {
    # Dot components would escape the per-label directory during cleanup.
    case "$1" in
        ''|.|..|*[!A-Za-z0-9._-]*) usage 'invalid label (use A-Z, a-z, 0-9, dot, underscore, or hyphen)' ;;
    esac
}

xml_string() {
    printf '    <string>'
    printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
    printf '</string>\n'
}

write_plist() {
    cat <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
PLIST
    xml_string "$label"
    printf '    <key>ProgramArguments</key>\n    <array>\n'
    local argument
    for argument in "$@"; do xml_string "$argument"; done
    printf '    </array>\n    <key>WorkingDirectory</key>\n'
    xml_string "$working_dir"
    printf '    <key>EnvironmentVariables</key>\n    <dict>\n    <key>PATH</key>\n'
    xml_string "$PATH"
    printf '    <key>HOME</key>\n'
    xml_string "${HOME:-}"
    printf '    </dict>\n    <key>StandardOutPath</key>\n'
    xml_string "$job_dir/out"
    printf '    <key>StandardErrorPath</key>\n'
    xml_string "$job_dir/err"
    cat <<'PLIST'
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><false/>
    <key>ProcessType</key><string>Interactive</string>
    <key>LimitLoadToSessionType</key><string>Aqua</string>
    <key>ExitTimeOut</key><integer>5</integer>
</dict>
</plist>
PLIST
}

mode=run
timeout=600
timeout_set=0
label=''
label_set=0
working_dir=$(pwd -P)
calling_dir=$working_dir
cwd_set=0
detach=0
dry_run=0
command_separator=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --timeout)
            [ "$#" -ge 2 ] || usage '--timeout needs a positive integer'
            timeout=$2
            timeout_set=1
            shift 2
            ;;
        --label)
            [ "$#" -ge 2 ] || usage '--label needs a value'
            label=$2
            label_set=1
            shift 2
            ;;
        --cwd)
            [ "$#" -ge 2 ] || usage '--cwd needs a directory'
            working_dir=$2
            cwd_set=1
            shift 2
            ;;
        --detach) detach=1; shift ;;
        --dry-run) dry_run=1; shift ;;
        --wait|--stop)
            [ "$mode" = run ] || usage 'choose only one operation'
            [ "$#" -ge 2 ] || usage "$1 needs a label"
            mode=${1#--}
            label=$2
            shift 2
            ;;
        --parse-status)
            [ "$mode" = run ] || usage 'choose only one operation'
            mode=parse-status
            shift
            ;;
        --)
            [ "$mode" = run ] || usage 'commands are only allowed in run mode'
            command_separator=1
            shift
            break
            ;;
        *) usage "unexpected argument: $1" ;;
    esac
done

case "$timeout" in ''|*[!0-9]*) usage '--timeout must be a positive integer' ;; esac
# Normalize leading zeroes without octal arithmetic (bash 3.2).
while [ "${#timeout}" -gt 1 ] && [ "${timeout#0}" != "$timeout" ]; do timeout=${timeout#0}; done
if ! [ "$timeout" -gt 0 ] 2>/dev/null; then usage '--timeout must be a positive integer'; fi

if [ "$mode" != run ]; then
    if [ "$label_set" -ne 0 ] || [ "$cwd_set" -ne 0 ] || [ "$detach" -ne 0 ] || [ "$dry_run" -ne 0 ]; then
        usage 'run options cannot be combined with --wait, --stop, or --parse-status'
    fi
    if [ "$mode" != wait ] && [ "$timeout_set" -ne 0 ]; then usage '--timeout is only valid for run or wait'; fi
fi
if [ "$mode" = parse-status ]; then
    parse_status
    exit 0
fi
if [ "$mode" = run ]; then
    [ "$command_separator" -eq 1 ] || usage 'missing -- before command'
    [ "$#" -gt 0 ] || usage 'missing command'
    if [ "$label_set" -eq 0 ]; then label="ai.gxlself.rs-bench.gui.$$.$RANDOM"; fi
fi
validate_label "$label"

state_dir=${RS_BENCH_GUI_DIR:-${TMPDIR:-/tmp}/rs-bench-gui}
case "$state_dir" in /*) ;; *) state_dir="$calling_dir/$state_dir" ;; esac
job_dir="$state_dir/$label"

if [ "$mode" = run ]; then
    if ! working_dir=$(cd -- "$working_dir" && pwd -P); then usage 'cannot access --cwd'; fi
    target_command=$1
    shift
    case "$target_command" in
        /*) executable=$target_command ;;
        */*) executable="$working_dir/$target_command" ;;
        *)
            executable=$(command -v -- "$target_command") || executable=''
            # command -v reports shell builtins by name; launchd needs an
            # external executable, e.g. /bin/echo for the builtin echo.
            case "$executable" in
                /*) ;;
                *) executable=$(type -P -- "$target_command") || executable='' ;;
            esac
            case "$executable" in
                ''|/*) ;;
                *) executable="$calling_dir/$executable" ;;
            esac
            ;;
    esac
    if [ ! -f "$executable" ] || [ ! -x "$executable" ]; then
        printf 'gui-run: command not found or not executable: %s\n' "$target_command" >&2
        exit 127
    fi
    # Canonicalize the parent to keep every ProgramArguments[0] absolute.
    executable_dir=$(cd -- "$(dirname -- "$executable")" && pwd -P)
    executable="$executable_dir/$(basename -- "$executable")"
    if [ "$dry_run" -eq 1 ]; then
        write_plist "$executable" "$@"
        exit 0
    fi
fi

domain="gui/$(id -u)"
service="$domain/$label"
if ! launchctl print "$domain" >/dev/null 2>&1; then
    printf 'gui-run: no logged-in Aqua graphical session for %s; log in at the Mac first\n' "$domain" >&2
    exit 125
fi

last_status=unknown
last_raw=''
read_status() {
    if last_raw=$(launchctl print "$service" 2>&1); then
        last_status=$(printf '%s\n' "$last_raw" | parse_status)
    else
        last_status=unknown
    fi
}

status_error() {
    printf 'gui-run: cannot parse launchctl status for %s (first 20 lines):\n' "$label" >&2
    printf '%s\n' "$last_raw" | sed -n '1,20p' >&2
}

bootout() {
    launchctl bootout "$service" >/dev/null 2>&1 || true
}

flush_and_remove() {
    if [ -f "$job_dir/out" ]; then cat "$job_dir/out" || true; fi
    if [ -f "$job_dir/err" ]; then cat "$job_dir/err" >&2 || true; fi
    rm -rf -- "$job_dir"
}

status_code() {
    case "$last_status" in
        'exit '*) printf '%s\n' "${last_status#exit }" ;;
        'signal '*) printf '%s\n' "$((128 + ${last_status#signal }))" ;;
        *) printf '143\n' ;;
    esac
}

if [ "$mode" = run ]; then
    umask 077
    mkdir -p -- "$state_dir"
    if ! mkdir -- "$job_dir"; then
        printf 'gui-run: state directory already exists or cannot be created: %s\n' "$job_dir" >&2
        exit 1
    fi
    # Resolve symlinks (e.g. /tmp) for launchd's output file paths.
    job_dir=$(cd -- "$job_dir" && pwd -P)
    : >"$job_dir/out"
    : >"$job_dir/err"
    write_plist "$executable" "$@" >"$job_dir/job.plist"
    if launchctl bootstrap "$domain" "$job_dir/job.plist"; then
        if [ "$detach" -eq 1 ]; then
            printf '%s\n' "$label"
            exit 0
        fi
    else
        bootstrap_code=$?
        flush_and_remove
        exit "$bootstrap_code"
    fi
fi

if [ "$mode" = stop ]; then
    read_status
    code=$(status_code)
    if [ "$last_status" = unknown ]; then status_error; fi
    bootout
    flush_and_remove
    exit "$code"
fi

wait_started=$SECONDS
while :; do
    read_status
    case "$last_status" in
        'exit '*|'signal '*)
            code=$(status_code)
            bootout
            flush_and_remove
            exit "$code"
            ;;
    esac
    if [ "$((SECONDS - wait_started))" -ge "$timeout" ]; then
        printf 'gui-run: timed out after %s seconds waiting for %s\n' "$timeout" "$label" >&2
        if [ "$last_status" = unknown ]; then status_error; fi
        bootout
        flush_and_remove
        exit 124
    fi
    sleep 0.2
done
