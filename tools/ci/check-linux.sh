#!/usr/bin/env bash
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Keep this list in sync with the section calls below. Frontend selectors such
# as "app", "host-ui", and "desktop" select every section with that prefix.
ALL_SECTIONS=(
    fmt
    clippy
    tests
    deny
    desktop-lock
    rustls-provider
    golint
    buflint
    "app install"
    "app typecheck"
    "app lint"
    "app test"
    kotlin
    android
    tailnet
    coverage
    "host-ui install"
    "host-ui typecheck"
    "host-ui lint"
    "host-ui format:check"
    "host-ui test"
    "desktop install"
    "desktop typecheck"
    "desktop lint"
    "desktop format:check"
    "desktop test"
    bench
    bindings
)
SECTION_GROUPS=(app host-ui desktop)
OPT_IN_SECTIONS=(kotlin android tailnet coverage)

usage() {
    cat <<'EOF'
Usage: tools/ci/check-linux.sh [--only SECTIONS | --list]

SECTIONS is a comma-separated list of section names. The app, host-ui and desktop
groups select every section for that frontend. --only takes precedence over
RS_CHECK_SECTIONS. Without a selector, every section runs.
kotlin, android, tailnet, and coverage are opt-in and only run when named
explicitly in --only or RS_CHECK_SECTIONS.
Lint tool lookup honors RS_LINT_TOOLS_DIR and RS_REQUIRE_LINT_TOOLS.
EOF
}

print_known_sections() {
    echo "Known sections:" >&2
    printf '  %s\n' "${ALL_SECTIONS[@]}" >&2
    echo "Known groups:" >&2
    printf '  %s\n' "${SECTION_GROUPS[@]}" >&2
}

trim() {
    local value=$1
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s' "$value"
}

selection=
selection_set=0
if [[ -n ${RS_CHECK_SECTIONS+x} ]]; then
    selection=$RS_CHECK_SECTIONS
    selection_set=1
fi
list_only=0
while (($#)); do
    case "$1" in
        --only)
            if (($# < 2)); then
                echo "--only requires a comma-separated section list" >&2
                print_known_sections
                exit 2
            fi
            selection=$2
            selection_set=1
            shift 2
            ;;
        --only=*)
            selection=${1#*=}
            selection_set=1
            shift
            ;;
        --list)
            list_only=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ((list_only)); then
    printf '%s\n' "${ALL_SECTIONS[@]}"
    exit 0
fi

select_all=0
selected_sections=()
if ((!selection_set)); then
    select_all=1
else
    if [[ -z $selection ]]; then
        echo "Section selector is empty" >&2
        print_known_sections
        exit 2
    fi
    IFS=',' read -r -a requested_sections <<< "$selection"
    unknown_sections=()
    for requested in "${requested_sections[@]}"; do
        name=$(trim "$requested")
        if [[ -z $name ]]; then
            echo "Empty section name in selector: $selection" >&2
            print_known_sections
            exit 2
        fi
        selected_sections+=("$name")

        known=0
        for known_name in "${ALL_SECTIONS[@]}"; do
            if [[ $name == "$known_name" ]]; then
                known=1
                break
            fi
        done
        if ((!known)); then
            for group in "${SECTION_GROUPS[@]}"; do
                if [[ $name == "$group" ]]; then
                    known=1
                    break
                fi
            done
        fi
        if ((!known)); then
            unknown_sections+=("$name")
        fi
    done

    if ((${#unknown_sections[@]})); then
        echo "Unknown section(s): ${unknown_sections[*]}" >&2
        print_known_sections
        exit 2
    fi
fi

is_opt_in() {
    local name=$1 candidate
    for candidate in "${OPT_IN_SECTIONS[@]}"; do
        [[ $name == "$candidate" ]] && return 0
    done
    return 1
}

is_selected() {
    local name=$1 selector
    if ((select_all)); then
        is_opt_in "$name" && return 1
        return 0
    fi
    for selector in "${selected_sections[@]}"; do
        if [[ $name == "$selector" || $name == "$selector "* ]]; then
            return 0
        fi
    done
    return 1
}

# Run every section and report all failures at the end. Stopping at the first
# failing section hid the frontend checks behind Rust test failures.
failed=()
section() {
    local name=$1
    shift
    if ! is_selected "$name"; then
        if ((select_all)) && is_opt_in "$name"; then
            echo "== $name (skipped: opt-in)"
        else
            echo "== $name (skipped)"
        fi
        return 0
    fi
    echo "== $name"
    "$@" && return 0
    failed+=("$name")
    return 1
}

section fmt cargo fmt --all -- --check

section clippy cargo clippy --workspace --all-targets -- -D warnings

section tests cargo nextest run --workspace --no-tests=pass --no-fail-fast

if [[ -d tools/bench/tests ]]; then
    section bench bash -c 'python3 -m unittest discover -s tools/bench/tests -t tools/bench && node --test tools/bench/page/test/*.test.js'
fi

if ! is_selected deny; then
    section deny true
elif command -v cargo-deny >/dev/null 2>&1; then
    # Machines without GitHub access set CARGO_DENY_OFFLINE=1 and use the cached advisory DB.
    if [[ ${CARGO_DENY_OFFLINE:-0} == 1 ]]; then
        section deny cargo deny --offline check
    else
        section deny cargo deny check
    fi
else
    echo "== deny"
    echo "cargo-deny not installed; skipping"
fi

# desktop/src-tauri has its own Cargo.lock; shared crates must match the root
# lock. Resync with `cp Cargo.lock desktop/src-tauri/Cargo.lock` and
# `cargo metadata` in desktop/src-tauri.
if [[ -f desktop/scripts/check-lock.sh ]]; then
    section desktop-lock bash desktop/scripts/check-lock.sh
fi

# Every rustls config names its crypto provider. Test builds with rs-rdp/testkit compile
# both ring and aws-lc-rs into rustls (ironrdp-server's tokio-rustls defaults), and the process-default constructors panic when both are on.
check_rustls_provider() {
    local hits
    hits=$(git grep -n -E '(ClientConfig|ServerConfig)::builder\(\)|builder_with_protocol_versions\(|WebPki(Server|Client)Verifier::builder\(|CryptoProvider::get_default\(|with_platform_verifier\(' -- '*.rs' || true)
    [[ -z $hits ]] && return 0
    echo "$hits"
    echo "use *_with_provider(Arc::new(rustls::crypto::ring::default_provider())) instead"
    return 1
}

section rustls-provider check_rustls_provider

lint_tool() {
    local name=$1 candidate
    if [[ -n ${RS_LINT_TOOLS_DIR:-} && -x "$RS_LINT_TOOLS_DIR/$name" ]]; then
        candidate="$RS_LINT_TOOLS_DIR/$name"
    elif candidate=$(command -v "$name" 2>/dev/null) && [[ -x $candidate ]]; then
        :
    elif [[ -x "$ROOT/target/ci-cache/lint-tools/bin/$name" ]]; then
        candidate="$ROOT/target/ci-cache/lint-tools/bin/$name"
    else
        return 1
    fi
    echo "lint tool $name: $candidate" >&2
    printf '%s' "$candidate"
}

run_golint() {
    local bin=$1 mod
    export GOLANGCI_LINT_CACHE="${GOLANGCI_LINT_CACHE:-$ROOT/target/ci-cache/golangci-lint}"
    mkdir -p "$GOLANGCI_LINT_CACHE"
    for mod in tailnet; do
        [[ -f "$mod/go.mod" ]] || continue
        echo "-- golangci-lint $mod"
        if ! (cd "$mod" && GOTOOLCHAIN=local "$bin" run ./...); then
            return 1
        fi
    done
}

run_bindings() {
    local status
    bash tools/ci/check-bindings.sh
    status=$?
    if ((status == 2)) && [[ ${RS_REQUIRE_APP_CHECKS:-0} != 1 ]]; then
        echo "bindings prerequisites missing; skipping"
        return 0
    fi
    return "$status"
}

if ! is_selected golint; then
    section golint true
else
    golangci_lint_bin=$(lint_tool golangci-lint || true)
    if [[ -n $golangci_lint_bin ]]; then
        section golint run_golint "$golangci_lint_bin"
    elif [[ ${RS_REQUIRE_LINT_TOOLS:-0} == 1 ]]; then
        echo "== golint"
        echo "golangci-lint not installed and RS_REQUIRE_LINT_TOOLS=1" >&2
        failed+=("golint tool-missing")
    else
        echo "== golint"
        echo "golangci-lint not installed; skipping (bash tools/ci/install-lint-tools.sh)"
    fi
fi

if ! is_selected buflint; then
    section buflint true
else
    buf_bin=$(lint_tool buf || true)
    if [[ -n $buf_bin ]]; then
        section buflint env BUF_CACHE_DIR="${BUF_CACHE_DIR:-$ROOT/target/ci-cache/buf}" "$buf_bin" lint
    elif [[ ${RS_REQUIRE_LINT_TOOLS:-0} == 1 ]]; then
        echo "== buflint"
        echo "buf not installed and RS_REQUIRE_LINT_TOOLS=1" >&2
        failed+=("buflint tool-missing")
    else
        echo "== buflint"
        echo "buf not installed; skipping (bash tools/ci/install-lint-tools.sh)"
    fi
fi

# Frontend projects: app/ (React Native), host-ui/ (Tauri web UI), and
# desktop/ (Tauri web UI).
frontend_check() {
    local dir=$1
    shift
    [[ -f $dir/package.json ]] || return 0
    # Skip partial frontend checkouts that do not have TypeScript configured.
    if [[ ! -f $dir/tsconfig.json ]]; then
        echo "== $dir: skipped (no tsconfig.json yet)"
        return 0
    fi
    local group_selected=0 name
    is_selected "$dir install" && group_selected=1
    for name in "$@"; do
        is_selected "$dir $name" && group_selected=1
    done
    if ((!group_selected)); then
        section "$dir install" true
        for name in "$@"; do
            section "$dir $name" true
        done
        return 0
    fi
    if command -v pnpm >/dev/null 2>&1; then
        section "$dir install" bash -c 'cd "$1" && pnpm install --frozen-lockfile' _ "$dir" || return 0
        local script
        for script in "$@"; do
            section "$dir $script" bash -c 'cd "$1" && pnpm "$2"' _ "$dir" "$script"
        done
    elif [[ ${RS_REQUIRE_APP_CHECKS:-0} == 1 ]]; then
        echo "== $dir"
        echo "pnpm not installed but RS_REQUIRE_APP_CHECKS=1" >&2
        failed+=("$dir pnpm-missing")
    else
        echo "== $dir"
        echo "pnpm not installed; skipping $dir checks"
    fi
}

frontend_check app typecheck lint test

gradle_home() {
    printf '%s' "${GRADLE_USER_HOME:-$ROOT/target/ci-cache/gradle}"
}

run_kotlin() {
    (cd app/android && GRADLE_USER_HOME="$(gradle_home)" ./gradlew -p nativelogic test --no-daemon)
}

android_missing_prerequisites() {
    local missing=() libraries=${RS_MOBILE_ANDROID_LIBS:-app/native/core/android/src/main/jniLibs}
    local abi architectures
    local -a abis=(arm64-v8a x86_64)

    command -v java >/dev/null 2>&1 || missing+=("java")
    if [[ -z ${ANDROID_HOME:-} && -z ${ANDROID_SDK_ROOT:-} ]]; then
        missing+=("ANDROID_HOME or ANDROID_SDK_ROOT")
    fi
    [[ -d app/node_modules ]] || missing+=("app/node_modules (run the app install section)")
    if [[ -f app/android/gradle.properties ]]; then
        architectures=$(sed -n 's/^reactNativeArchitectures[[:space:]]*=[[:space:]]*//p' app/android/gradle.properties | tail -n 1)
        if [[ -n $architectures ]]; then
            IFS=',' read -r -a abis <<< "$architectures"
        fi
    fi
    if [[ ! -d $libraries ]]; then
        missing+=("RS_MOBILE_ANDROID_LIBS directory ($libraries)")
    else
        for abi in "${abis[@]}"; do
            abi=$(trim "$abi")
            [[ -f "$libraries/$abi/librs_mobile.a" ]] || missing+=("$libraries/$abi/librs_mobile.a")
        done
    fi

    ((${#missing[@]})) || return 1
    local IFS='; '
    printf '%s' "${missing[*]}"
}

run_android() {
    (cd app/android && GRADLE_USER_HOME="$(gradle_home)" \
        ./gradlew :app:assembleDebug :app:lintDebug --no-daemon)
}

tailnet_missing_prerequisites() {
    local missing=() tool
    local -a required_tools=(go cc)
    for tool in "${required_tools[@]}"; do
        command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
    done
    ((${#missing[@]})) || return 1
    local IFS='; '
    printf '%s' "${missing[*]}"
}

run_tailnet() {
    local output summary lib testnet

    (cd tailnet && go vet ./... && go test ./...) || return 1

    if ! output=$(RS_TAILNET_OUT="$ROOT/target/ci-cache/tailnet-out" bash tailnet/scripts/build-host.sh); then
        printf '%s\n' "$output"
        return 1
    fi
    printf '%s\n' "$output"
    summary=$(printf '%s\n' "$output" | tail -n 1)
    if [[ $summary != *'; lib='*'; testnet='* ]]; then
        echo "could not parse tailnet build paths from: $summary" >&2
        return 1
    fi
    lib=${summary#*'; lib='}
    lib=${lib%%'; testnet='*}
    testnet=${summary##*'; testnet='}
    if [[ -z $lib || -z $testnet ]]; then
        echo "tailnet build returned an empty library or testnet path" >&2
        return 1
    fi

    RS_TAILNET_LIB="$lib" RS_TAILNET_TESTNET="$testnet" RS_REQUIRE_TAILNET=1 \
        cargo nextest run -p rs-tailnet -p rs-mobile --no-fail-fast \
        -E 'package(rs-tailnet) | binary(/tailnet/)'
}

if ! is_selected kotlin; then
    section kotlin true
elif ! command -v java >/dev/null 2>&1; then
    echo "== kotlin"
    if [[ ${RS_REQUIRE_ANDROID_CHECKS:-0} == 1 ]]; then
        echo "java not installed and RS_REQUIRE_ANDROID_CHECKS=1" >&2
        failed+=("kotlin tool-missing")
    else
        echo "java not installed; skipping kotlin"
    fi
else
    section kotlin run_kotlin
fi

if ! is_selected android; then
    section android true
elif android_missing=$(android_missing_prerequisites); then
    echo "== android"
    if [[ ${RS_REQUIRE_ANDROID_CHECKS:-0} == 1 ]]; then
        echo "missing Android prerequisites: $android_missing; RS_REQUIRE_ANDROID_CHECKS=1" >&2
        failed+=("android tool-missing")
    else
        echo "missing Android prerequisites: $android_missing; skipping android"
    fi
else
    section android run_android
fi

if ! is_selected tailnet; then
    section tailnet true
elif tailnet_missing=$(tailnet_missing_prerequisites); then
    echo "== tailnet"
    if [[ ${RS_REQUIRE_TAILNET_CHECKS:-0} == 1 ]]; then
        echo "missing tailnet prerequisites: $tailnet_missing; RS_REQUIRE_TAILNET_CHECKS=1" >&2
        failed+=("tailnet tool-missing")
    else
        echo "missing tailnet prerequisites: $tailnet_missing; skipping tailnet"
    fi
else
    section tailnet run_tailnet
fi

if ! is_selected coverage; then
    section coverage true
elif ! cargo llvm-cov --version >/dev/null 2>&1; then
    echo "== coverage"
    if [[ ${RS_REQUIRE_COVERAGE:-0} == 1 ]]; then
        echo "cargo-llvm-cov not installed and RS_REQUIRE_COVERAGE=1" >&2
        failed+=("coverage tool-missing")
    else
        echo "cargo-llvm-cov not installed; skipping coverage"
    fi
else
    section coverage cargo llvm-cov nextest -p rsp-core --summary-only --fail-under-lines 85
fi

frontend_check host-ui typecheck lint format:check test

frontend_check desktop typecheck lint format:check test

section bindings run_bindings

if ((${#failed[@]})); then
    echo "CHECK FAILED: ${failed[*]}"
    exit 1
fi
echo "CHECK OK"
