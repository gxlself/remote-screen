#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo="$(cd "$script_dir/../../.." && pwd -P)"
source "$script_dir/common.sh"
mkdir -p "$repo/target"
temp="$(mktemp -d "$repo/target/android-release-test.XXXXXX")"
cleanup() { rm -rf -- "$temp"; }
trap cleanup EXIT

for key in BUILD_SECRETS_PROVIDER BUILD_SECRETS_SYNCED NACOS_CONFIG_FILE NACOS_SERVER NACOS_NAMESPACE NACOS_GROUP NACOS_DATA_ID NACOS_USERNAME NACOS_PASSWORD; do
    unset "$key"
done

passed=0
total=0
ok() { passed=$((passed + 1)); total=$((total + 1)); printf 'ok - %s\n' "$1"; }
not_ok() { total=$((total + 1)); printf 'not ok - %s\n' "$1"; exit 1; }
check() { local name="$1"; shift; if "$@"; then ok "$name"; else not_ok "$name"; fi; }

members="$temp/members.txt"
cat >"$members" <<'MEMBERS'
AndroidManifest.xml
lib/arm64-v8a/librs_tailnet.so
lib/arm64-v8a/librs_tailnet.so.bak
lib/x86_64/librs_tailnet.so
MEMBERS
check 'tailnet state reports all ABIs present' test "$(tailnet_state_from_members "$members" arm64-v8a x86_64)" = on

printf '%s\n' AndroidManifest.xml >"$members"
check 'tailnet state reports no ABI present' test "$(tailnet_state_from_members "$members" arm64-v8a x86_64)" = off

printf '%s\n' 'lib/arm64-v8a/librs_tailnet.so' >"$members"
check 'tailnet state reports partial ABI coverage' test "$(tailnet_state_from_members "$members" arm64-v8a x86_64)" = partial

printf '%s\n' 'lib/arm64-v8a/librs_tailnet.so.bak' >"$members"
check 'tailnet state ignores similarly named backup' test "$(tailnet_state_from_members "$members" arm64-v8a x86_64)" = off

check 'tailnet gate accepts on' tailnet_release_gate on true false
check 'tailnet gate accepts unknown' tailnet_release_gate unknown false false
check 'tailnet gate accepts off by default' tailnet_release_gate off false false
set +e
gate_output="$(tailnet_release_gate off true false 2>&1)"
gate_status=$?
set -e
check 'tailnet gate rejects off when tailnet was asked for' test "$gate_status" -ne 0
check 'tailnet gate rejection names allow flag' grep -q -- '--allow-no-tailnet' <<<"$gate_output"
set +e
gate_output="$(tailnet_release_gate off true true 2>&1)"
gate_status=$?
set -e
check 'tailnet gate allows off explicitly' test "$gate_status" -eq 0
check 'tailnet gate allow emits warning' grep -q -- '--allow-no-tailnet' <<<"$gate_output"
set +e
tailnet_release_gate partial true true >/dev/null 2>&1
gate_status=$?
set -e
check 'tailnet gate always rejects partial' test "$gate_status" -ne 0

# Run the release wrapper with build/SDK stubs to verify provider selection and
# the summary produced by a complete local or optional-provider release.
release_repo="$temp/release-repo"
release_tools="$release_repo/tools/release"
fake_bin="$temp/bin"
sdk="$temp/sdk"
mkdir -p "$release_tools/android" "$release_repo/app/scripts" "$release_repo/app/android/app" \
    "$release_repo/app/node_modules/@react-native/gradle-plugin" "$fake_bin" "$sdk/build-tools/35.0.0" \
    "$sdk/ndk/27.0.0/toolchains/llvm/prebuilt/linux-x86_64/bin"
cp "$repo/tools/release/android.sh" "$release_tools/android.sh"
cp "$script_dir/common.sh" "$release_tools/android/common.sh"
cp "$repo/app/scripts/build-secrets-config.sh" "$release_repo/app/scripts/"
printf 'ndkVersion = "27.0.0"\n' >"$release_repo/app/android/build.gradle"
for tool in keytool jarsigner pnpm; do
    printf '#!/bin/sh\nexit 0\n' >"$fake_bin/$tool"
    chmod 755 "$fake_bin/$tool"
done
for tool in apksigner zipalign aapt2; do
    cp "$fake_bin/keytool" "$sdk/build-tools/35.0.0/$tool"
done
cp "$fake_bin/keytool" "$sdk/ndk/27.0.0/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf"
cat >"$fake_bin/java" <<'SCRIPT'
#!/bin/sh
echo 'openjdk version "17.0.1"' >&2
SCRIPT
cat >"$fake_bin/zipinfo" <<'SCRIPT'
#!/bin/sh
echo 'AndroidManifest.xml'
SCRIPT
chmod 755 "$fake_bin/java" "$fake_bin/zipinfo"
cat >"$release_repo/app/android/gradlew" <<'SCRIPT'
#!/usr/bin/env bash
set -eu
state=missing
[[ -f keystore.local.properties ]] && state=ready
printf 'RS-VERSION {"versionName":"1.0.0","versionCode":1000000,"buildNumber":0,"signing":"%s","signingSource":"local"}\n' "$state"
SCRIPT
chmod 755 "$release_repo/app/android/gradlew"
mkdir -p "$release_repo/app/android/app/build/outputs/"{bundle,apk}/release
: >"$release_repo/app/android/app/build/outputs/bundle/release/app-release.aab"
: >"$release_repo/app/android/app/build/outputs/apk/release/app-release.apk"
cat >"$release_tools/android/verify.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -eu
while (($#)); do
    if [[ "$1" == --json ]]; then
        printf '{"abis":["arm64-v8a"],"signerSha256":"test-signer","pageSize16k":"pass"}\n' >"$2"
        exit 0
    fi
    shift
done
exit 1
SCRIPT
chmod 755 "$release_tools/android/verify.sh"
: >"$release_repo/app/android/keystore.local.properties"
run_release_case() {
    local name="$1"; shift
    release_status=0
    PATH="$fake_bin:$PATH" ANDROID_HOME="$sdk" \
        bash "$release_tools/android.sh" --skip-rust --skip-js-install --skip-build --out "$temp/$name-out" "$@" \
        >"$temp/$name-output" 2>&1 || release_status=$?
}
run_release_case local-default
check 'Android release defaults to local files without a pull script' test "$release_status" -eq 0
check 'Android local summary records signing source' python3 - "$temp/local-default-out/release-summary.json" <<'PY'
import json, sys
assert json.load(open(sys.argv[1]))['secretsSource'] == 'local-files'
PY
export BUILD_SECRETS_PROVIDER=nacos
run_release_case missing-provider
check 'Android requested provider rejects missing coordinates' test "$release_status" -ne 0
check 'Android missing provider diagnostic names keys' grep -q 'Nacos requested; missing NACOS_SERVER' "$temp/missing-provider-output"
export NACOS_CONFIG_FILE="$temp/android-provider.env"
cat >"$NACOS_CONFIG_FILE" <<'CONFIG'
NACOS_SERVER=https://config.example.com
NACOS_NAMESPACE=release-team
NACOS_GROUP=custom-release-group
NACOS_DATA_ID=custom-signing.properties
CONFIG
cat >"$release_repo/app/scripts/nacos-build-secrets.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -eu
[[ "$*" == 'pull --platform android --require-android-signing' ]]
[[ "$NACOS_GROUP" == 'custom-release-group' && "$NACOS_DATA_ID" == 'custom-signing.properties' ]]
printf 'pull\n' >>"$FAKE_SECRETS_LOG"
SCRIPT
export FAKE_SECRETS_LOG="$temp/secrets.log"
run_release_case optional-provider
check 'Android release accepts custom coordinates in named env file' test "$release_status" -eq 0
check 'Android optional provider is pulled once' test "$(wc -l <"$FAKE_SECRETS_LOG")" -eq 1
check 'Android optional-provider summary records signing source' python3 - "$temp/optional-provider-out/release-summary.json" <<'PY'
import json, sys
assert json.load(open(sys.argv[1]))['secretsSource'] == 'nacos'
PY
export NACOS_CONFIG_FILE="$temp/missing.env"
run_release_case force-local --skip-secrets-pull
check 'Android skip-secrets-pull ignores optional provider config' test "$release_status" -eq 0
check 'Android forced local does not pull again' test "$(wc -l <"$FAKE_SECRETS_LOG")" -eq 1
run_release_case explicit-properties --keystore-properties "$release_repo/app/android/keystore.local.properties"
check 'explicit Android properties override optional provider' test "$release_status" -eq 0
unset BUILD_SECRETS_PROVIDER NACOS_CONFIG_FILE FAKE_SECRETS_LOG
rm "$release_repo/app/android/keystore.local.properties"
run_release_case missing-signing
check 'Android default still rejects missing local signing' test "$release_status" -ne 0
check 'Android missing signing diagnostic names properties' grep -q MYAPP_UPLOAD_STORE_FILE "$temp/missing-signing-output"

printf 'PASS %d/%d\n' "$passed" "$total"
