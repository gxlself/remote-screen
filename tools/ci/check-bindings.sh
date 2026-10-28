#!/usr/bin/env bash
# Compare the committed UniFFI bindings with bindings generated from rs-mobile.
#
# The files under app/src/native/generated and app/cpp/generated are produced by
# ubrn and committed to the repository. This check builds the Linux rs-mobile
# library, generates into app/artifacts/bindcheck.XXXXXX, and compares the two
# trees. app/artifacts is intentionally below app/ so ubrn finds the same
# node_modules prettier and app/.prettierrc.js as the Mac build does.
#
# C++ is normalized before comparison because the committed output was formatted
# on a Mac with clang-format, while this Linux check does not require it. The
# normalization first merges adjacent string literals with a character state
# machine, then removes comments, sorts #include lines, and removes whitespace
# from the remaining text. Raw strings R"(...)" are not supported and are
# treated as ordinary strings; generated bindings do not use them.
#
# Fix stale bindings on a Mac with:
#   pnpm --dir app bindings
# then commit app/src/native/generated and app/cpp/generated. Do not regenerate
# or reformat the committed bindings on Linux.
set -uo pipefail

usage() {
    printf 'Usage: %s [--keep]\n' "$(basename "$0")"
}

keep=0
while (($#)); do
    case "$1" in
        --keep)
            keep=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
    shift
done

SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)" || {
    printf 'BINDINGS SKIP: cannot resolve script directory\n' >&2
    exit 2
}
ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)" || {
    printf 'BINDINGS SKIP: cannot resolve repository root\n' >&2
    exit 2
}
APP="${RS_APP_ROOT:-$ROOT/app}"

skip() {
    printf 'BINDINGS SKIP: %s\n' "$*" >&2
    exit 2
}

resolve_path() {
    local path=$1
    if [[ "$path" != /* ]]; then
        path="$PWD/$path"
    fi
    printf '%s/%s\n' "$(cd -- "$(dirname -- "$path")" 2>/dev/null && pwd)" "$(basename -- "$path")"
}

if ! command -v python3 >/dev/null 2>&1; then
    skip 'python3 is required'
fi

if ! cd -- "$APP" 2>/dev/null; then
    skip "app root not found: $APP"
fi
APP="$(pwd)"
UBRN="${RS_UBRN:-$APP/node_modules/.bin/ubrn}"
if [[ "$UBRN" != /* ]]; then
    UBRN="$PWD/$UBRN"
fi
PRETTIER="$APP/node_modules/.bin/prettier"
if [[ ! -x "$UBRN" ]]; then
    skip "ubrn not found or not executable: $UBRN (run: pnpm --dir app install --frozen-lockfile)"
fi
if [[ ! -x "$PRETTIER" ]]; then
    skip "prettier not found or not executable: $PRETTIER (run: pnpm --dir app install --frozen-lockfile)"
fi

requested_lib="${RS_BINDINGS_LIB:-}"
if [[ -n "$requested_lib" ]]; then
    if [[ "$requested_lib" != /* ]]; then
        requested_lib="$ROOT/$requested_lib"
    fi
    requested_lib="$(resolve_path "$requested_lib")"
    LIB="$requested_lib"
    if [[ ! -f "$LIB" ]]; then
        skip "RS_BINDINGS_LIB is not a file: $LIB"
    fi
else
    if ! command -v cargo >/dev/null 2>&1; then
        skip 'cargo is required to build rs-mobile'
    fi
    target_dir="${CARGO_TARGET_DIR:-$ROOT/target}"
    if [[ "$target_dir" != /* ]]; then
        target_dir="$ROOT/$target_dir"
    fi
    LIB="$target_dir/debug/librs_mobile.a"
fi

if [[ -z "$requested_lib" ]]; then
    if ! (cd -- "$ROOT" && CARGO_TARGET_DIR="$target_dir" cargo build --locked -p rs-mobile --lib); then
        printf 'BINDINGS FAIL: cargo build -p rs-mobile failed\n'
        exit 1
    fi
    if [[ ! -f "$LIB" ]]; then
        printf 'BINDINGS FAIL: cargo build -p rs-mobile failed\n'
        printf 'reason: static library not found: %s\n' "$LIB"
        exit 1
    fi
else
    LIB="$(cd -- "$(dirname -- "$LIB")" && pwd)/$(basename -- "$LIB")"
fi

mkdir -p -- "$APP/artifacts" || skip "cannot create $APP/artifacts"
STAGE="$(mktemp -d "$APP/artifacts/bindcheck.XXXXXX")" || skip 'cannot create a binding staging directory'

cleanup() {
    if ((keep)); then
        printf 'kept staging directory: %s\n' "$STAGE"
    else
        rm -rf -- "$STAGE"
    fi
}
trap cleanup EXIT

if [[ ! -d "$APP/src/native/generated" || ! -d "$APP/cpp/generated" ]]; then
    skip "committed binding directories not found under $APP"
fi

if ! (cd -- "$ROOT" && "$UBRN" generate jsi bindings --library \
    --ts-dir "$STAGE/ts" --cpp-dir "$STAGE/cpp" "$LIB") \
    >"$STAGE/ubrn.log" 2>"$STAGE/ubrn.stderr.log"; then
    cat -- "$STAGE/ubrn.stderr.log" >>"$STAGE/ubrn.log"
    {
        printf 'BINDINGS SKIP: ubrn generate jsi bindings failed (network access may be required for the first ubrn_cli build)\n'
        tail -n 20 "$STAGE/ubrn.stderr.log"
    } >&2
    exit 2
fi
cat -- "$STAGE/ubrn.stderr.log" >>"$STAGE/ubrn.log"
generated_ts_count="$(find "$STAGE/ts" -type f 2>/dev/null | wc -l | tr -d ' ')"
generated_cpp_count="$(find "$STAGE/cpp" -type f 2>/dev/null | wc -l | tr -d ' ')"
if [[ "$generated_ts_count" == 0 || "$generated_cpp_count" == 0 ]]; then
    {
        printf 'BINDINGS SKIP: ubrn returned success but generated no TS or C++ files\n'
        tail -n 20 "$STAGE/ubrn.stderr.log"
    } >&2
    exit 2
fi

compare_output="$(python3 - "$APP" "$STAGE" <<'PY'
from pathlib import Path
import difflib
import os
import sys


def regular_files(root):
    found = {}
    root = Path(root)
    if not root.is_dir():
        return found
    for base, dirs, names in os.walk(root):
        dirs.sort()
        names.sort()
        for name in names:
            path = Path(base) / name
            if path.is_file() and not path.is_symlink():
                found[path.relative_to(root).as_posix()] = path
    return found


def app_relative(full_path, app):
    try:
        return Path(full_path).relative_to(app).as_posix()
    except ValueError:
        return Path(full_path).as_posix()


def differs_without_whitespace(repo_text, generated_text):
    """Compare text while ignoring CR/LF, spaces, and tabs."""
    return repo_text.translate({'\r': None, '\n': None, ' ': None, '\t': None}) != \
        generated_text.translate({'\r': None, '\n': None, ' ': None, '\t': None})


def merge_adjacent_string_literals(text):
    """Merge "a" "b" into "ab" without touching escaped quotes or character literals."""
    out = []
    state = 'normal'
    # string_start indexes the opening quote in out; string_value excludes quotes.
    string_value = []
    string_start = 0
    i = 0
    while i < len(text):
        char = text[i]
        following = text[i + 1] if i + 1 < len(text) else ''
        if state == 'normal':
            if char == '/' and following == '/':
                state = 'line_comment'
                out.append(char)
                out.append(following)
                i += 2
                continue
            if char == '/' and following == '*':
                state = 'block_comment'
                out.append(char)
                out.append(following)
                i += 2
                continue
            if char == '"':
                state = 'string'
                string_value = []
                string_start = len(out)
                out.append(char)
                i += 1
                continue
            elif char == "'":
                state = 'character'
            out.append(char)
            i += 1
            continue
        if state == 'line_comment':
            out.append(char)
            if char == '\n':
                state = 'normal'
            i += 1
            continue
        if state == 'block_comment':
            out.append(char)
            if char == '*' and following == '/':
                out.append(following)
                i += 2
                state = 'normal'
            else:
                i += 1
            continue
        if char == '\\' and i + 1 < len(text):
            if state == 'string':
                string_value.append(char)
                string_value.append(text[i + 1])
            out.append(char)
            out.append(text[i + 1])
            i += 2
            continue
        if state == 'string' and char != '"':
            string_value.append(char)
            out.append(char)
            i += 1
            continue
        if state == 'string' and char == '"':
            next_char = i + 1
            while next_char < len(text) and text[next_char] in ' \t\r\n':
                next_char += 1
            if next_char < len(text) and text[next_char] == '"':
                replacement = '"' + ''.join(string_value)
                out[string_start:] = [replacement]
                i = next_char + 1
                state = 'string'
                string_value = []
                string_start = len(out) - len(replacement)
                continue
            out.append(char)
            state = 'normal'
            i += 1
            continue
        if state == 'character' and char == "'":
            state = 'normal'
        out.append(char)
        i += 1
    return ''.join(out)


def strip_cpp_comments(text):
    """Remove C++ comments while preserving string and character literals."""
    out = []
    state = 'normal'
    i = 0
    while i < len(text):
        char = text[i]
        following = text[i + 1] if i + 1 < len(text) else ''
        if state == 'normal':
            if char == '/' and following == '/':
                state = 'line_comment'
                i += 2
                continue
            if char == '/' and following == '*':
                state = 'block_comment'
                i += 2
                continue
            if char == '"':
                state = 'string'
            elif char == "'":
                state = 'character'
            if char != '\r':
                out.append(char)
            i += 1
            continue
        if state == 'line_comment':
            if char == '\n':
                out.append(char)
                state = 'normal'
            i += 1
            continue
        if state == 'block_comment':
            if char == '*' and following == '/':
                out.append(' ')
                state = 'normal'
                i += 2
            else:
                i += 1
            continue
        if char != '\r':
            out.append(char)
        if char == '\\':
            if i + 1 < len(text):
                if text[i + 1] != '\r':
                    out.append(text[i + 1])
                i += 2
                continue
        elif (state == 'string' and char == '"') or (state == 'character' and char == "'"):
            state = 'normal'
        i += 1
    return ''.join(out)


def normalized_cpp(text):
    uncommented = strip_cpp_comments(merge_adjacent_string_literals(text))
    includes = []
    body = []
    for line in uncommented.splitlines(keepends=True):
        stripped = line.strip()
        if stripped.startswith('#include'):
            includes.append(stripped)
        else:
            body.append(line)
    return sorted(includes), ''.join(body).replace(' ', '').replace('\t', '').replace('\n', '').replace('\r', '')


def show_cpp_body_difference(repo_text, generated_text, details):
    limit = min(len(repo_text), len(generated_text))
    position = next((i for i in range(limit) if repo_text[i] != generated_text[i]), limit)
    start = max(0, position - 80)
    end = position + 80
    details.append(f'first difference at normalized offset {position} (80 characters of context)')
    details.append(f'repo:      {repo_text[start:end]!r}')
    details.append(f'generated: {generated_text[start:end]!r}')


def show_line_difference(repo_text, generated_text, details):
    repo_lines = repo_text.splitlines()
    generated_lines = generated_text.splitlines()
    position = len(repo_lines)
    for index in range(min(len(repo_lines), len(generated_lines))):
        if repo_lines[index] != generated_lines[index]:
            position = index
            break
    start = max(0, position - 4)
    end = position + 5
    details.append(f'first difference at line {position + 1} (10 lines of context)')
    for line in difflib.unified_diff(
        repo_lines[start:end],
        generated_lines[start:end],
        fromfile=f'repo line {start + 1}',
        tofile=f'generated line {start + 1}',
        lineterm='',
    ):
        details.append(line)


def main():
    app = Path(sys.argv[1])
    stage = Path(sys.argv[2])
    ts_generated = regular_files(stage / 'ts')
    cpp_generated = regular_files(stage / 'cpp')
    ts_repo = regular_files(app / 'src/native/generated')
    cpp_repo = regular_files(app / 'cpp/generated')
    stale = False

    def report(reason, relative, details=()):
        nonlocal stale
        stale = True
        print(f'STALE {reason} {relative}')
        for detail in details:
            print(detail)

    for relative in sorted(set(ts_generated) | set(ts_repo)):
        if relative not in ts_repo:
            report('missing-in-repo', f'src/native/generated/{relative}')
        elif relative not in ts_generated:
            report('extra-in-repo', f'src/native/generated/{relative}')

    for relative in sorted(set(cpp_generated) | set(cpp_repo)):
        if relative not in cpp_repo:
            report('missing-in-repo', f'cpp/generated/{relative}')
        elif relative not in cpp_generated:
            report('extra-in-repo', f'cpp/generated/{relative}')
        elif Path(relative).suffix in {'.cpp', '.hpp', '.h'}:
            repo_path = cpp_repo[relative]
            generated_path = cpp_generated[relative]
            repo_includes, repo_body = normalized_cpp(repo_path.read_text(errors='replace'))
            generated_includes, generated_body = normalized_cpp(generated_path.read_text(errors='replace'))
            relative_to_app = app_relative(repo_path, app)
            details = []
            if repo_includes == generated_includes and repo_body == generated_body:
                continue
            if repo_includes != generated_includes:
                details.append('sorted #include lines differ')
                repo_include_set = set(repo_includes)
                generated_include_set = set(generated_includes)
                for include in repo_includes:
                    if include not in generated_include_set:
                        details.append(f'- {include}')
                for include in generated_includes:
                    if include not in repo_include_set:
                        details.append(f'+ {include}')
            repo_raw = repo_path.read_text(errors='replace')
            generated_raw = generated_path.read_text(errors='replace')
            if repo_body != generated_body and differs_without_whitespace(repo_raw, generated_raw):
                details.append('normalized C++ body differs (comments and whitespace ignored)')
                show_line_difference(repo_raw, generated_raw, details)
                show_cpp_body_difference(repo_body, generated_body, details)
            if details:
                report('content', relative_to_app, details)

    for relative in sorted(set(ts_generated) | set(ts_repo)):
        if relative not in ts_generated or relative not in ts_repo:
            continue
        repo_bytes = ts_repo[relative].read_bytes()
        generated_bytes = ts_generated[relative].read_bytes()
        if repo_bytes == generated_bytes:
            continue
        repo_text = repo_bytes.decode('utf-8', errors='replace').splitlines()
        generated_text = generated_bytes.decode('utf-8', errors='replace').splitlines()
        diff = list(difflib.unified_diff(
            repo_text,
            generated_text,
            fromfile=app_relative(ts_repo[relative], app),
            tofile=f'src/native/generated/{relative}',
            lineterm='',
        ))
        report('content', app_relative(ts_repo[relative], app), diff[:40])

    if stale:
        print('BINDINGS STALE: committed UniFFI bindings do not match crates/rs-mobile')
        print('fix: on a Mac run "pnpm --dir app bindings" (app/scripts/generate-bindings.sh) and commit app/src/native/generated and app/cpp/generated')
        return 1
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception as error:
        print(f'bindings comparison failed: {error}', file=sys.stderr)
        sys.exit(3)
PY
)"
compare_status=$?

if ((compare_status == 0)); then
    printf 'BINDINGS OK (ts=%s files, cpp=%s files)\n' "$generated_ts_count" "$generated_cpp_count"
elif ((compare_status == 1)); then
    printf '%s\n' "$compare_output"
else
    printf '%s\n' "$compare_output"
    printf 'BINDINGS FAIL: binding comparison failed\n'
fi

exit "$compare_status"
