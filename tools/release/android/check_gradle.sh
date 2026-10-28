#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(cd "$script_dir/../../.." && pwd -P)
android_dir="$repo_root/app/android"
# Keep generated keystores and logs in the configured scratch directory.
if [[ "${CARGO_HOME:-}" == */cargo ]]; then
    tmp_root="$(cd "$(dirname "$CARGO_HOME")" && pwd -P)/tmp"
else
    tmp_root="$repo_root/target"
fi
mkdir -p "$tmp_root"
umask 077
work=$(mktemp -d "$tmp_root/rs-gradle-check.XXXXXX")
gradle_daemon_dir="$work/gradle-daemons"
mkdir -p "$gradle_daemon_dir"
gradle_base_args=(--no-daemon -q "-Dorg.gradle.daemon.registry.base=$gradle_daemon_dir")
trap 'rm -rf -- "$work"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

for tool in awk node keytool openssl; do
    command -v "$tool" >/dev/null || fail "required tool not found: $tool"
done
store_password=$(openssl rand -hex 16)
key_password=$(openssl rand -hex 16)

check_no_passwords() {
    if grep -F -q -e "$store_password" -e "$key_password" "$1"; then
        fail "$(basename "$1"): generated signing password appeared in output (output suppressed)"
    fi
}

run_ok() {
    local label=$1 log=$2
    shift 2
    if (cd "$android_dir" && ./gradlew "${gradle_base_args[@]}" "$@") >"$log" 2>&1; then
        check_no_passwords "$log"
    else
        check_no_passwords "$log"
        cat "$log" >&2
        fail "$label: Gradle should have succeeded"
    fi
}

run_fail() {
    local label=$1 log=$2 explanation=$3
    shift 3
    if (cd "$android_dir" && ./gradlew "${gradle_base_args[@]}" "$@") >"$log" 2>&1; then
        check_no_passwords "$log"
        fail "$label: Gradle should have failed"
    fi
    check_no_passwords "$log"
    if ! grep -F -q -- "$explanation" "$log"; then
        cat "$log" >&2
        fail "$label: missing explanation: $explanation"
    fi
    printf 'PASS: %s\n' "$label"
}

assert_version() {
    local label=$1 log=$2 version=$3 code=$4 build=$5 signing=${6:-} source=${7:-}
    if ! node - "$log" "$version" "$code" "$build" "$signing" "$source" <<'JS'
const fs = require('fs');
const [log, versionName, versionCode, buildNumber, signing, signingSource] = process.argv.slice(2);
const lines = fs.readFileSync(log, 'utf8').split(/\r?\n/).filter(line => line.startsWith('RS-VERSION '));
if (lines.length !== 1) process.exit(1);
const actual = JSON.parse(lines[0].slice('RS-VERSION '.length));
const expected = {versionName, versionCode: Number(versionCode), buildNumber: Number(buildNumber)};
if (signing) expected.signing = signing;
if (signingSource) expected.signingSource = signingSource;
if (Object.entries(expected).some(([key, value]) => actual[key] !== value)) process.exit(1);
JS
    then
        cat "$log" >&2
        fail "$label: unexpected RS-VERSION JSON"
    fi
    printf 'PASS: %s\n' "$label"
}

# Read only workspace.package.version, independently from the Gradle parser.
cargo_version=$(awk '
    /^[[:space:]]*\[workspace\.package\][[:space:]]*(#.*)?$/ { in_package = 1; next }
    /^[[:space:]]*\[/ { in_package = 0 }
    in_package && /^[[:space:]]*version[[:space:]]*=/ {
        split($0, parts, "\""); print parts[2]; exit
    }
' "$repo_root/Cargo.toml")
[[ "$cargo_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'Cargo workspace version is not x.y.z'
IFS=. read -r major minor patch <<<"$cargo_version"
default_code=$((10#$major * 1000000 + 10#$minor * 10000 + 10#$patch * 100))

run_ok 'default version' "$work/default.log" :app:rsPrintVersion
assert_version 'default version matches Cargo.toml' "$work/default.log" "$cargo_version" "$default_code" 0
grep '^RS-VERSION ' "$work/default.log"
run_ok 'build number override' "$work/build.log" -PrsBuildNumber=7 :app:rsPrintVersion
assert_version 'build number adds 7' "$work/build.log" "$cargo_version" "$((default_code + 7))" 7
run_ok 'version override' "$work/override.log" -PrsVersionName=1.2.3 -PrsBuildNumber=4 :app:rsPrintVersion
assert_version '1.2.3 build 4 maps to 1020304' "$work/override.log" 1.2.3 1020304 4

run_fail 'reject incomplete version' "$work/invalid-version.log" "Invalid rsVersionName '1.2' from -PrsVersionName" -PrsVersionName=1.2 :app:rsPrintVersion
run_fail 'reject minor above 99' "$work/invalid-minor.log" "Invalid rsVersionName '0.100.0' from -PrsVersionName" -PrsVersionName=0.100.0 :app:rsPrintVersion
run_fail 'reject patch above 99' "$work/invalid-patch.log" "Invalid rsVersionName '0.0.100' from -PrsVersionName" -PrsVersionName=0.0.100 :app:rsPrintVersion
run_fail 'reject build above 99' "$work/invalid-build.log" "Invalid rsBuildNumber '100' from -PrsBuildNumber" -PrsBuildNumber=100 :app:rsPrintVersion
run_fail 'reject negative build' "$work/negative-build.log" "Invalid rsBuildNumber '-1' from -PrsBuildNumber" -PrsBuildNumber=-1 :app:rsPrintVersion
run_fail 'reject versionCode above Play limit' "$work/play-overflow.log" 'rsVersionCode exceeds Play limit 2100000000' -PrsVersionName=2100.0.0 -PrsBuildNumber=1 :app:rsPrintVersion
run_fail 'reject major without integer overflow' "$work/major-overflow.log" 'rsVersionCode exceeds Play limit 2100000000' -PrsVersionName=99999999999999999999.0.0 :app:rsPrintVersion

# Never create or modify the release owner's local keystore*.properties files.
empty_properties="$work/empty.properties"
: >"$empty_properties"
for task in assembleRelease bundleRelease; do
    log="$work/missing-$task.log"
    run_fail "missing signing rejects $task" "$log" 'Release signing is not configured (property): missing' "-PrsKeystoreProperties=$empty_properties" --dry-run ":app:$task"
    for key in MYAPP_UPLOAD_STORE_FILE MYAPP_UPLOAD_KEY_ALIAS MYAPP_UPLOAD_STORE_PASSWORD MYAPP_UPLOAD_KEY_PASSWORD; do
        grep -F -q "$key" "$log" || fail "$task: missing signing diagnostic omitted $key"
    done
    grep -F -q 'bash app/scripts/nacos-build-secrets.sh pull --platform android --require-android-signing' "$log" || fail "$task: missing recovery command"
done
run_ok 'debug without release signing' "$work/debug.log" "-PrsKeystoreProperties=$empty_properties" --dry-run :app:assembleDebug
printf 'PASS: debug does not require release signing\n'
run_ok 'missing signing status' "$work/missing-status.log" "-PrsKeystoreProperties=$empty_properties" :app:rsPrintVersion
assert_version 'missing signing status' "$work/missing-status.log" "$cargo_version" "$default_code" 0 missing property

# JKS supports separate store and key passwords; both are generated for this run.
if ! keytool -genkeypair -storetype JKS -keystore "$work/t.jks" -alias rs-test \
    -keyalg RSA -keysize 2048 -validity 1 -dname 'CN=rs-test' \
    -storepass "$store_password" -keypass "$key_password" >"$work/keytool.log" 2>&1; then
    check_no_passwords "$work/keytool.log"
    cat "$work/keytool.log" >&2
    fail 'temporary keystore generation'
fi
check_no_passwords "$work/keytool.log"
signing_properties="$work/signing.properties"
cat >"$signing_properties" <<PROPERTIES
MYAPP_UPLOAD_STORE_FILE=$work/t.jks
MYAPP_UPLOAD_KEY_ALIAS=rs-test
MYAPP_UPLOAD_STORE_PASSWORD=$store_password
MYAPP_UPLOAD_KEY_PASSWORD=$key_password
PROPERTIES
run_ok 'ready signing status' "$work/ready-status.log" "-PrsKeystoreProperties=$signing_properties" :app:rsPrintVersion
assert_version 'ready signing status' "$work/ready-status.log" "$cargo_version" "$default_code" 0 ready property
run_ok 'release tasks with signing' "$work/ready-release.log" "-PrsKeystoreProperties=$signing_properties" --dry-run :app:assembleRelease :app:bundleRelease
printf 'PASS: release APK and bundle task graphs accept temporary signing\n'
for log in "$work"/*.log; do
    check_no_passwords "$log"
done
printf 'PASS: no generated signing passwords in any output\n'
printf 'Android Gradle release checks passed.\n'
