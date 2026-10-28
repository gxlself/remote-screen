#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo="$(cd "$script_dir/../../.." && pwd -P)"
source "$script_dir/common.sh"
temp="$(mktemp -d "${TMPDIR:-/tmp}/ios-release-test.XXXXXX")"
cleanup() { rm -rf -- "$temp"; }
trap cleanup EXIT

for key in BUILD_SECRETS_PROVIDER BUILD_SECRETS_SYNCED NACOS_CONFIG_FILE NACOS_SERVER NACOS_NAMESPACE NACOS_GROUP NACOS_DATA_ID NACOS_USERNAME NACOS_PASSWORD; do
    unset "$key"
done

workspace_version="$(read_workspace_version "$repo/Cargo.toml")"
workspace_bundle0="$(bundle_version "$workspace_version" 0)"
workspace_bundle3="$(bundle_version "$workspace_version" 3)"
workspace_bundle7="$(bundle_version "$workspace_version" 7)"

passed=0
total=0
ok() { passed=$((passed + 1)); total=$((total + 1)); printf 'ok - %s\n' "$1"; }
not_ok() { total=$((total + 1)); printf 'not ok - %s\n' "$1"; exit 1; }
check() { local name="$1"; shift; if "$@"; then ok "$name"; else not_ok "$name"; fi; }

tailnet_xcframework="$temp/RsTailnet.xcframework"
mkdir -p "$tailnet_xcframework"
check 'tailnet gate accepts xcframework path' tailnet_release_gate "$tailnet_xcframework" true false
set +e
tailnet_gate_output="$(tailnet_release_gate off false false 2>&1)"
tailnet_gate_status=$?
set -e
check 'tailnet gate accepts off by default (App Store)' test "$tailnet_gate_status" -eq 0
check 'tailnet gate records that the build has no tailnet' grep -q 'does not include the in-app tailnet' <<<"$tailnet_gate_output"
set +e
tailnet_gate_output="$(tailnet_release_gate off true false 2>&1)"
tailnet_gate_status=$?
set -e
check 'tailnet gate rejects off when --with-tailnet was asked' test "$tailnet_gate_status" -ne 0
check 'tailnet gate rejection names allow flag' grep -q -- '--allow-no-tailnet' <<<"$tailnet_gate_output"
set +e
tailnet_gate_output="$(tailnet_release_gate off true true 2>&1)"
tailnet_gate_status=$?
set -e
check 'tailnet gate allows off explicitly' test "$tailnet_gate_status" -eq 0
check 'tailnet gate allow emits warning' grep -q -- '--allow-no-tailnet' <<<"$tailnet_gate_output"
check 'tailnet gate accepts unknown' tailnet_release_gate unknown true false
tailnet_gate_output="$(tailnet_release_gate unknown false false 2>&1)"
check 'tailnet gate warns that a skipped Rust build is unconfirmed' grep -q 'cannot confirm' <<<"$tailnet_gate_output"

cargo_a="$temp/Cargo-a.toml"
cat >"$cargo_a" <<'TOML'
[package]
version = "9.9.9"
[workspace.package]
version = "0.2.3"
[dependencies]
version = "8.8.8"
TOML
cargo_b="$temp/Cargo-b.toml"
printf '[package]\nversion = "0.2.3"\n' >"$cargo_b"
check 'read_workspace_version uses workspace package' test "$(read_workspace_version "$cargo_a")" = 0.2.3
set +e
read_workspace_version "$cargo_b" >/dev/null
status=$?
set -e
check 'read_workspace_version missing section fails' test "$status" -ne 0
check 'bundle_version 0.1.0/0' test "$(bundle_version 0.1.0 0)" = 10000
check 'bundle_version 0.2.3/7' test "$(bundle_version 0.2.3 7)" = 20307
check 'bundle_version 1.0.0/99' test "$(bundle_version 1.0.0 99)" = 1000099
check 'bundle_version maximum valid' test "$(bundle_version 2099.99.99 99)" = 2099999999
for pair in '0.100.0 0' '1.2 0' '0.1.0 100' '2100.0.0 0'; do
    read -r pair_version pair_build <<<"$pair"
    set +e
    bundle_version "$pair_version" "$pair_build" >/dev/null 2>&1
    status=$?
    set -e
    check "bundle_version rejects $pair" test "$status" -ne 0
done

props="$temp/fake.properties"
cat >"$props" <<'PROPS'
# comment

NACOS_GROUP = "remote-screen" # route
ASC_API_KEY_ID=ABCDE12345
IOS_ASC_API_KEY_P8_BASE64=ZmFrZQ==
ASC_API_ISSUER_ID = '00000000-0000-0000-0000-000000000000' # uuid
PROPS
check 'read_prop only requested key' test "$(read_prop "$props" ASC_API_KEY_ID)" = ABCDE12345
check 'read_prop strips quote and comment' test "$(read_prop "$props" ASC_API_ISSUER_ID)" = 00000000-0000-0000-0000-000000000000
check 'read_prop does not print unrelated values' test "$(read_prop "$props" ASC_API_KEY_ID)" != *ZmFrZQ==*
check 'valid team and key ids' valid_team_id ABCDE12345
check 'valid issuer id' valid_issuer_id 00000000-0000-0000-0000-000000000000
for value in abcde12345 ABCDE1234 ABCDE123456; do
    set +e; valid_team_id "$value"; status=$?; set -e
    check "invalid id $value" test "$status" -ne 0
done
set +e; valid_issuer_id not-a-uuid; status=$?; set -e
check 'invalid issuer id' test "$status" -ne 0

mask_input="$temp/mask.txt"
printf 'team=ABCDE12345 key=QWERTY6789 issuer=00000000-0000-0000-0000-000000000000\nkeep=unchanged\n' >"$mask_input"
export RS_MASK_TEAM=ABCDE12345 RS_MASK_KEY=QWERTY6789 RS_MASK_ISSUER=00000000-0000-0000-0000-000000000000
mask_file "$mask_input"
check 'mask_file replaces all values' bash -c '! grep -Eq "ABCDE12345|QWERTY6789|00000000-0000" "$1"' _ "$mask_input"
check 'mask_file preserves other text' grep -q 'keep=unchanged' "$mask_input"
printf 'ABCDE12345 QWERTY6789 00000000-0000-0000-0000-000000000000\n' | mask_stream >"$temp/masked-stream"
check 'mask_stream replaces all values' bash -c '! grep -Eq "ABCDE12345|QWERTY6789|00000000-0000" "$1"' _ "$temp/masked-stream"
unset RS_MASK_TEAM RS_MASK_KEY RS_MASK_ISSUER
printf 'unchanged\n' >"$mask_input"
mask_file "$mask_input"
check 'empty masks leave file unchanged' grep -qx unchanged "$mask_input"

for destination in export upload; do
    export_plist="$temp/$destination.plist"
    write_export_options "$export_plist" ABCDE12345 "$destination"
    check "write_export_options $destination fields" python3 - "$export_plist" "$destination" <<'PY'
import pathlib
import plistlib
import sys
data = plistlib.loads(pathlib.Path(sys.argv[1]).read_bytes())
expected = {'method': 'app-store-connect', 'destination': sys.argv[2], 'teamID': 'ABCDE12345', 'signingStyle': 'automatic', 'uploadSymbols': True, 'manageAppVersionAndBuildNumber': False, 'stripSwiftSymbols': True}
raise SystemExit(0 if data == expected else 1)
PY
done
set +e
write_export_options "$temp/no.plist" ABCDE12345 foo >/dev/null 2>&1
status=$?
set -e
check 'write_export_options rejects destination' test "$status" -ne 0
check 'write_export_options does not create invalid file' test ! -e "$temp/no.plist"

make_archive() {
    local root="$1" encryption="${2:-false}" version="${3:-$workspace_version}" bundle="${4:-$workspace_bundle7}"
    local app="$root/X.xcarchive/Products/Applications/RemoteScreen.app"
    mkdir -p "$app" "$root/X.xcarchive/dSYMs/RemoteScreen.app.dSYM"
    python3 - "$app/Info.plist" "$encryption" "$version" "$bundle" <<'PY'
import plistlib
import sys
info = {'CFBundleIdentifier': 'ai.gxlself.remotescreen', 'CFBundleShortVersionString': sys.argv[3], 'CFBundleVersion': sys.argv[4], 'DTPlatformName': 'iphoneos', 'MinimumOSVersion': '16.0', 'CFBundleExecutable': 'RemoteScreen'}
if sys.argv[2] == 'true': info['ITSAppUsesNonExemptEncryption'] = False
with open(sys.argv[1], 'wb') as stream: plistlib.dump(info, stream)
PY
printf '#!/bin/sh\n' >"$app/RemoteScreen"
printf 'privacy' >"$app/PrivacyInfo.xcprivacy"
printf 'js' >"$app/main.jsbundle"
}
fake_bin="$temp/fake-bin"
mkdir -p "$fake_bin"
cat >"$fake_bin/lipo" <<'SCRIPT'
#!/bin/sh
printf 'arm64\n'
SCRIPT
chmod 755 "$fake_bin/lipo"
export PATH="$fake_bin:$PATH"
base="$temp/base"
make_archive "$base"
archive="$base/X.xcarchive"
verify_output="$temp/verify-output"
set +e
"$script_dir/verify.sh" archive "$archive" --version "$workspace_version" --bundle-version "$workspace_bundle7" --unsigned >"$verify_output" 2>&1
status=$?
set -e
check 'archive verification passes' test "$status" -eq 0
check 'archive verification warns encryption' grep -q '^WARN ITSAppUsesNonExemptEncryption' "$verify_output"
check 'archive verification writes json' test -f "$base/verify-archive.json"
for mutation in bundle-id version build privacy js dsym arch; do
    root="$temp/m-$mutation"
    cp -a "$base" "$root"
    app="$root/X.xcarchive/Products/Applications/RemoteScreen.app"
    case "$mutation" in
        bundle-id) python3 - "$app/Info.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'rb') as f: p=plistlib.load(f)
p['CFBundleIdentifier']='bad.id'
with open(sys.argv[1], 'wb') as f: plistlib.dump(p, f)
PY
            expected='CFBundleIdentifier' ;;
        version) python3 - "$app/Info.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'rb') as f: p=plistlib.load(f)
p['CFBundleShortVersionString']='9.9.9'
with open(sys.argv[1], 'wb') as f: plistlib.dump(p, f)
PY
            expected='CFBundleShortVersionString' ;;
        build) python3 - "$app/Info.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'rb') as f: p=plistlib.load(f)
p['CFBundleVersion']='9'
with open(sys.argv[1], 'wb') as f: plistlib.dump(p, f)
PY
            expected='CFBundleVersion' ;;
        privacy) rm "$app/PrivacyInfo.xcprivacy"; expected='PrivacyInfo.xcprivacy' ;;
        js) rm "$app/main.jsbundle"; expected='main.jsbundle' ;;
        dsym) rm -rf "$root/X.xcarchive/dSYMs/RemoteScreen.app.dSYM"; expected='RemoteScreen.app.dSYM' ;;
        arch) cat >"$fake_bin/lipo" <<'SCRIPT'
#!/bin/sh
printf 'arm64 x86_64\n'
SCRIPT
            chmod 755 "$fake_bin/lipo"; expected='arm64 executable' ;;
    esac
    set +e
    out_text="$("$script_dir/verify.sh" archive "$root/X.xcarchive" --version "$workspace_version" --bundle-version "$workspace_bundle7" --unsigned 2>&1)"
    status=$?
    set -e
    check "archive mutation $mutation fails" test "$status" -eq 1
    check "archive mutation $mutation reports failure" grep -q "FAIL $expected" <<<"$out_text"
    if [[ "$mutation" == arch ]]; then
        cat >"$fake_bin/lipo" <<'SCRIPT'
#!/bin/sh
printf 'arm64\n'
SCRIPT
        chmod 755 "$fake_bin/lipo"
    fi
done

plan_output="$temp/plan.txt"
set +e
"$repo/tools/release/ios.sh" --plan --dry-run >"$plan_output" 2>&1
status=$?
set -e
check 'dry-run plan succeeds' test "$status" -eq 0
check 'dry-run plan contains archive' grep -q 'xcodebuild archive' "$plan_output"
check 'dry-run plan contains signing disabled' grep -q 'CODE_SIGNING_ALLOWED=NO' "$plan_output"
check 'dry-run plan contains version settings' grep -q "MARKETING_VERSION=$workspace_version" "$plan_output"
check 'dry-run plan contains bundle setting' grep -q "CURRENT_PROJECT_VERSION=$workspace_bundle0" "$plan_output"
check 'dry-run plan omits export and authentication' bash -c '! grep -Eq "exportArchive|authenticationKey" "$1"' _ "$plan_output"
check 'default plan builds Rust without tailnet' grep -q 'RS_TAILNET_IOS=off CONFIGURATION=Release' "$plan_output"
check 'default plan says the build has no tailnet' grep -q 'tailnet: not included' "$plan_output"
tailnet_plan="$temp/tailnet-plan.txt"
"$repo/tools/release/ios.sh" --plan --dry-run --with-tailnet >"$tailnet_plan" 2>&1
check 'with-tailnet plan builds tailnet' grep -q 'build tailnet if needed' "$tailnet_plan"
check 'with-tailnet plan says tailnet is included' grep -q 'tailnet: included' "$tailnet_plan"
RS_TAILNET_IOS="$tailnet_xcframework" "$repo/tools/release/ios.sh" --plan --dry-run >"$tailnet_plan" 2>&1
check 'prebuilt RS_TAILNET_IOS opts in' grep -q 'tailnet: included' "$tailnet_plan"
team_plan="$temp/team-plan.txt"
set +e
RS_IOS_TEAM_ID=ABCDE12345 "$repo/tools/release/ios.sh" --plan --skip-secrets-pull --upload --build 3 >"$team_plan" 2>&1
status=$?
set -e
check 'upload plan succeeds' test "$status" -eq 0
check 'upload plan includes export and upload' grep -q 'exportArchive' "$team_plan"
check 'upload plan masks team' bash -c '! grep -q ABCDE12345 "$1"' _ "$team_plan"
check 'upload plan uses build 03' grep -q "CURRENT_PROJECT_VERSION=$workspace_bundle3" "$team_plan"
"$repo/tools/release/ios.sh" --plan >"$team_plan" 2>&1
check 'default plan uses local signing files' grep -q 'signing materials: use local' "$team_plan"
NACOS_CONFIG_FILE="$temp/not-readable.env" "$repo/tools/release/ios.sh" --plan >"$team_plan" 2>&1
check 'remote plan describes optional sync without reading secrets' grep -q 'nacos-build-secrets.sh pull' "$team_plan"
NACOS_CONFIG_FILE="$temp/not-readable.env" "$repo/tools/release/ios.sh" --plan --dry-run >"$team_plan" 2>&1
check 'dry-run plan ignores provider configuration' grep -q 'signing materials: skipped (dry-run)' "$team_plan"

# Exercise the real release flow with platform/tool stubs.  The temporary
# repository layout keeps ios.sh's repo_root() behavior intact while allowing
# verify.sh and xcodebuild to model both failures and a successful upload.
release_repo="$temp/release-repo"
release_tools="$release_repo/tools/release"
release_ios="$release_tools/ios"
mkdir -p "$release_ios" "$release_repo/app/scripts" "$release_repo/app/ios/RemoteScreen.xcodeproj"
cp "$repo/tools/release/ios.sh" "$release_tools/ios.sh"
cp "$repo/tools/release/ios/common.sh" "$release_ios/common.sh"
cp "$repo/app/scripts/build-secrets-config.sh" "$release_repo/app/scripts/build-secrets-config.sh"
cp "$repo/app/scripts/app-size.py" "$release_repo/app/scripts/app-size.py"
cp "$repo/Cargo.toml" "$release_repo/Cargo.toml"
: >"$release_repo/app/ios/RemoteScreen.xcodeproj/project.pbxproj"
cat >"$release_repo/app/ios/asc-api-key.properties" <<'PROPS'
ASC_API_KEY_ID=KEYID12345
ASC_API_ISSUER_ID=00000000-0000-0000-0000-000000000000
PROPS
printf 'fake key\n' >"$release_repo/app/AuthKey_KEYID12345.p8"
chmod 600 "$release_repo/app/AuthKey_KEYID12345.p8"
mkdir -p "$release_repo/app/rust" "$release_repo/tailnet/scripts"
cat >"$release_repo/app/rust/build-ios.sh" <<'SCRIPT'
#!/usr/bin/env bash
printf 'RS_TAILNET_IOS=%s\n' "${RS_TAILNET_IOS-<unset>}" >>"$FAKE_RUST_LOG"
echo "rs-mobile: release; tailnet=${RS_TAILNET_IOS:-auto}"
SCRIPT
chmod 755 "$release_repo/app/rust/build-ios.sh"
cat >"$release_repo/tailnet/scripts/build-ios.sh" <<'SCRIPT'
#!/usr/bin/env bash
printf 'tailnet build\n' >>"$FAKE_RUST_LOG"
mkdir -p "$RS_TAILNET_OUT/ios/RsTailnet.xcframework"
SCRIPT
chmod 755 "$release_repo/tailnet/scripts/build-ios.sh"
cat >"$release_ios/verify.sh" <<'SCRIPT'
#!/bin/sh
mode=$1
printf 'fake verify %s\n' "$mode"
if [ "${FAKE_VERIFY_FAIL:-}" = "$mode" ]; then
    exit 1
fi
exit 0
SCRIPT
chmod 755 "$release_ios/verify.sh"
release_fake_bin="$temp/release-fake-bin"
mkdir -p "$release_fake_bin"
cat >"$release_fake_bin/xcodebuild" <<'SCRIPT'
#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$FAKE_XCODEBUILD_LOG"
if [ "${1:-}" = -version ]; then
    printf 'Xcode 16.0\n'
    exit 0
fi
if [ "${1:-}" = archive ]; then
    archive_path=''
    version=''
    bundle=''
    previous=''
    for arg in "$@"; do
        if [ "$previous" = -archivePath ]; then archive_path=$arg; fi
        case "$arg" in
            MARKETING_VERSION=*) version=${arg#*=} ;;
            CURRENT_PROJECT_VERSION=*) bundle=${arg#*=} ;;
        esac
        previous=$arg
    done
    app="$archive_path/Products/Applications/RemoteScreen.app"
    mkdir -p "$app" "$archive_path/dSYMs/RemoteScreen.app.dSYM"
    python3 - "$app/Info.plist" "$version" "$bundle" <<'PY'
import plistlib
import sys
info = {
    'CFBundleIdentifier': 'ai.gxlself.remotescreen',
    'CFBundleShortVersionString': sys.argv[2],
    'CFBundleVersion': sys.argv[3],
    'DTPlatformName': 'iphoneos',
    'MinimumOSVersion': '16.0',
    'CFBundleExecutable': 'RemoteScreen',
}
with open(sys.argv[1], 'wb') as stream:
    plistlib.dump(info, stream)
PY
    : >"$app/RemoteScreen"
    printf 'privacy\n' >"$app/PrivacyInfo.xcprivacy"
    printf 'js\n' >"$app/main.jsbundle"
    exit 0
fi
if [ "${1:-}" = -exportArchive ]; then
    export_path=''
    previous=''
    for arg in "$@"; do
        if [ "$previous" = -exportPath ]; then export_path=$arg; fi
        previous=$arg
    done
    mkdir -p "$export_path"
    : >"$export_path/RemoteScreen.ipa"
    case "$export_path" in
        */upload) printf 'upload\n' >>"$FAKE_UPLOAD_LOG" ;;
    esac
    exit 0
fi
exit 0
SCRIPT
chmod 755 "$release_fake_bin/xcodebuild"
cat >"$release_fake_bin/ditto" <<'SCRIPT'
#!/bin/sh
set -eu
output=''
for arg in "$@"; do output=$arg; done
: >"$output"
SCRIPT
chmod 755 "$release_fake_bin/ditto"
for tool in xcrun plutil codesign security lipo rustup; do
    cat >"$release_fake_bin/$tool" <<'SCRIPT'
#!/bin/sh
exit 0
SCRIPT
    chmod 755 "$release_fake_bin/$tool"
done
cat >"$release_fake_bin/uname" <<'SCRIPT'
#!/bin/sh
printf 'Darwin\n'
SCRIPT
chmod 755 "$release_fake_bin/uname"

run_release_case() {
    local name="$1" fail_mode="$2" upload="$3"; shift 3
    local output="$temp/$name-output" out_dir="$temp/$name-out"
    local upload_log="$temp/$name-upload.log" xcode_log="$temp/$name-xcodebuild.log"
    local rust_log="$temp/$name-rust.log"
    : >"$upload_log"
    : >"$xcode_log"
    : >"$rust_log"
    local args=(--skip-js-install --skip-pods --out "$out_dir")
    [[ "$upload" == true ]] && args+=(--upload)
    # Extra arguments (at least one) replace the default --skip-rust.
    if (($# > 0)); then args+=("$@"); else args+=(--skip-rust); fi
    set +e
    PATH="$release_fake_bin:$PATH" \
        FAKE_VERIFY_FAIL="$fail_mode" FAKE_UPLOAD_LOG="$upload_log" FAKE_XCODEBUILD_LOG="$xcode_log" \
        FAKE_RUST_LOG="$rust_log" \
        RS_IOS_TEAM_ID=ABCDE12345 "$release_tools/ios.sh" "${args[@]}" >"$output" 2>&1
    release_status=$?
    set -e
    printf '%s\n' "$release_status" >"$temp/$name-status"
}

run_release_case archive-verification-failure archive true
archive_failure_status="$(cat "$temp/archive-verification-failure-status")"
check 'archive verification failure exits nonzero' test "$archive_failure_status" -ne 0
check 'archive verification failure is reported' grep -q 'Archive verification failed' "$temp/archive-verification-failure-output"
check 'archive verification failure does not upload' test ! -s "$temp/archive-verification-failure-upload.log"

run_release_case ipa-verification-failure ipa true
ipa_failure_status="$(cat "$temp/ipa-verification-failure-status")"
check 'IPA verification failure exits nonzero' test "$ipa_failure_status" -ne 0
check 'IPA verification failure is reported' grep -q 'IPA verification failed' "$temp/ipa-verification-failure-output"
check 'IPA verification failure does not upload' test ! -s "$temp/ipa-verification-failure-upload.log"

run_release_case normal-upload '' true
normal_status="$(cat "$temp/normal-upload-status")"
check 'normal upload path succeeds' test "$normal_status" -eq 0
check 'normal path allows provisioning updates' grep -q -- '-allowProvisioningUpdates' "$temp/normal-upload-xcodebuild.log"
check 'normal path passes API key path' grep -q -- '-authenticationKeyPath .*AuthKey_KEYID12345.p8' "$temp/normal-upload-xcodebuild.log"
check 'normal path passes key id' grep -q -- '-authenticationKeyID KEYID12345' "$temp/normal-upload-xcodebuild.log"
check 'normal path passes issuer id' grep -q -- '-authenticationKeyIssuerID 00000000-0000-0000-0000-000000000000' "$temp/normal-upload-xcodebuild.log"
check 'normal path passes team' grep -q -- 'DEVELOPMENT_TEAM=ABCDE12345' "$temp/normal-upload-xcodebuild.log"
check 'normal path uploads exactly once' test "$(wc -l <"$temp/normal-upload-upload.log")" -eq 1
check 'skip-rust summary leaves tailnet unknown' python3 - "$temp/normal-upload-out/release-summary.json" <<'PY'
import json, sys
summary = json.load(open(sys.argv[1]))
raise SystemExit(0 if summary['tailnet'] == 'unknown' and summary['tailnetIncluded'] is None and summary['secretsSource'] == 'local-files' else 1)
PY

cat >"$release_repo/app/scripts/nacos-build-secrets.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -eu
[[ "$*" == 'pull --platform ios' ]]
[[ "$NACOS_GROUP" == 'custom-release-group' && "$NACOS_DATA_ID" == 'custom-signing.properties' ]]
printf 'pull\n' >>"$FAKE_SECRETS_LOG"
SCRIPT
export FAKE_SECRETS_LOG="$temp/secrets.log" BUILD_SECRETS_PROVIDER=nacos
run_release_case missing-provider-coordinates '' false
check 'requested iOS provider rejects missing coordinates' test "$(cat "$temp/missing-provider-coordinates-status")" -ne 0
check 'iOS provider diagnostic names missing coordinates' grep -q 'Nacos requested; missing NACOS_SERVER' "$temp/missing-provider-coordinates-output"
check 'invalid provider does not upload' test ! -s "$temp/missing-provider-coordinates-upload.log"
export NACOS_CONFIG_FILE="$temp/ios-provider.env"
cat >"$NACOS_CONFIG_FILE" <<'CONFIG'
NACOS_SERVER=https://config.example.com
NACOS_NAMESPACE=release-team
NACOS_GROUP=custom-release-group
NACOS_DATA_ID=custom-signing.properties
CONFIG
run_release_case optional-provider '' false
check 'iOS release accepts custom provider coordinates from named file' test "$(cat "$temp/optional-provider-status")" -eq 0
check 'iOS release invokes optional pull once' test "$(wc -l <"$FAKE_SECRETS_LOG")" -eq 1
check 'iOS summary identifies optional provider' python3 - "$temp/optional-provider-out/release-summary.json" <<'PY'
import json, sys
assert json.load(open(sys.argv[1]))['secretsSource'] == 'nacos'
PY
export NACOS_CONFIG_FILE="$temp/not-readable.env"
run_release_case force-local '' false --skip-rust --skip-secrets-pull
check 'iOS skip-secrets-pull ignores optional provider config' test "$(cat "$temp/force-local-status")" -eq 0
check 'forced local release does not pull again' test "$(wc -l <"$FAKE_SECRETS_LOG")" -eq 1
unset BUILD_SECRETS_PROVIDER NACOS_CONFIG_FILE FAKE_SECRETS_LOG

unset RS_TAILNET_IOS
run_release_case app-store '' false --build 0
app_store_status="$(cat "$temp/app-store-status")"
check 'App Store release without tailnet succeeds' test "$app_store_status" -eq 0
check 'App Store release builds Rust with tailnet off' grep -qx 'RS_TAILNET_IOS=off' "$temp/app-store-rust.log"
check 'App Store release skips the tailnet build' bash -c '! grep -q "tailnet build" "$1"' _ "$temp/app-store-rust.log"
check 'App Store release logs that tailnet is left out' grep -q 'does not include the in-app tailnet' "$temp/app-store-output"
check 'App Store summary records no tailnet' python3 - "$temp/app-store-out/release-summary.json" <<'PY'
import json, sys
summary = json.load(open(sys.argv[1]))
raise SystemExit(0 if summary['tailnet'] == 'off' and summary['tailnetIncluded'] is False else 1)
PY

run_release_case with-tailnet '' false --with-tailnet
with_tailnet_status="$(cat "$temp/with-tailnet-status")"
check 'with-tailnet release succeeds' test "$with_tailnet_status" -eq 0
check 'with-tailnet release builds tailnet first' test "$(head -n1 "$temp/with-tailnet-rust.log")" = 'tailnet build'
check 'with-tailnet release links the built xcframework' grep -q '^RS_TAILNET_IOS=.*/RsTailnet.xcframework$' "$temp/with-tailnet-rust.log"
check 'with-tailnet summary records tailnet' python3 - "$temp/with-tailnet-out/release-summary.json" <<'PY'
import json, sys
summary = json.load(open(sys.argv[1]))
raise SystemExit(0 if summary['tailnetIncluded'] is True and summary['tailnet'].endswith('RsTailnet.xcframework') else 1)
PY

for args in '--dry-run --upload' '--build 100' '--build x' '--team-id bad' '--unknown'; do
    set +e; # shellcheck disable=SC2086
    "$repo/tools/release/ios.sh" $args >/dev/null 2>&1
    status=$?
    set -e
    check "argument error $args" test "$status" -eq 2
done
if [[ "$(uname -s)" != Darwin ]]; then
    set +e
    output="$($repo/tools/release/ios.sh --dry-run 2>&1)"
    status=$?
    set -e
    check 'dry-run on Linux rejected' test "$status" -eq 1
    check 'Linux rejection names macOS' grep -q 'requires macOS' <<<"$output"
else
    ok 'Linux-only dry-run rejection skipped on Darwin'
fi
check 'help succeeds' "$repo/tools/release/ios.sh" --help
trace_pattern='set'" "'-x'
check 'no shell tracing' bash -c 'pattern="$1"; shift; ! grep -n "$pattern" "$@"' _ "$trace_pattern" "$repo/tools/release/ios.sh" "$script_dir/common.sh" "$script_dir/verify.sh" "$script_dir/test.sh"
secret_pattern='source .*asc-api-'"key|\\. .*asc-api-"'key'
check 'no secret source' bash -c 'pattern="$1"; shift; ! grep -Eq "$pattern" "$@"' _ "$secret_pattern" "$repo/tools/release/ios.sh" "$script_dir"/*.sh
p8_pattern='cat .*\\.'"p8"
check 'no p8 cat' bash -c 'pattern="$1"; shift; ! grep -Eq "$pattern" "$@"' _ "$p8_pattern" "$repo/tools/release/ios.sh" "$script_dir"/*.sh
printf 'PASS %d/%d\n' "$passed" "$total"
