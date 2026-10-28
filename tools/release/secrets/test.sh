#!/usr/bin/env bash
# Exercise local signing and the optional provider without contacting a server.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
temp="$(mktemp -d "${TMPDIR:-/tmp}/build-secrets-test.XXXXXX")"
trap 'rm -rf -- "$temp"' EXIT
for key in BUILD_SECRETS_PROVIDER BUILD_SECRETS_SYNCED NACOS_CONFIG_FILE NACOS_SERVER NACOS_NAMESPACE NACOS_GROUP NACOS_DATA_ID NACOS_USERNAME NACOS_PASSWORD; do
    unset "$key"
done
export NACOS_CURL_RETRIES=1 NACOS_CURL_RETRY_DELAY=0
passed=0
check() {
    local name="$1"; shift
    if "$@"; then
        passed=$((passed + 1))
        printf 'ok - %s\n' "$name"
    else
        printf 'not ok - %s\n' "$name" >&2
        exit 1
    fi
}
run() {
    status=0
    "$@" >"$temp/output" 2>&1 || status=$?
}

app="$temp/app"
mkdir -p "$app/scripts" "$app/android/app" "$app/ios" "$temp/bin"
cp "$repo/app/scripts/"{build-secrets-config,nacos-build-secrets}.sh "$app/scripts/"
script="$app/scripts/nacos-build-secrets.sh"
cat >"$app/android/keystore.local.properties" <<'PROPS'
MYAPP_UPLOAD_STORE_FILE=app/release.keystore
MYAPP_UPLOAD_KEY_ALIAS=release
MYAPP_UPLOAD_STORE_PASSWORD=test-store-password
MYAPP_UPLOAD_KEY_PASSWORD=test-key-password
PROPS
printf 'test keystore' >"$app/android/app/release.keystore"
# An unnamed conventional env file must not silently enable a remote provider.
printf 'NACOS_SERVER=unexpected.invalid\n' >"$app/scripts/nacos-build-secrets.env"

export FAKE_CURL_LOG="$temp/curl.log" FAKE_REMOTE="$temp/remote.properties" FAKE_PUBLISHED="$temp/published.properties"
cat >"$temp/bin/curl" <<'PY'
#!/usr/bin/env python3
import os
import pathlib
import sys

args = sys.argv[1:]
data = dict(item.split('=', 1) for i, item in enumerate(args) if i and args[i-1] == '--data-urlencode' and '=' in item)
login = any('/v1/auth/login' in arg for arg in args)
publish = '-X' in args and args[args.index('-X')+1] == 'POST' and not login
with open(os.environ['FAKE_CURL_LOG'], 'a') as log:
    log.write('login\n' if login else 'publish\n' if publish else 'fetch\n')
if os.environ.get('FAKE_CURL_FAILURE'):
    sys.exit(7)
if login:
    assert data['username'] == os.environ['NACOS_USERNAME']
    assert data['password'] == os.environ['NACOS_PASSWORD']
    print('{"accessToken":"test-token"}')
else:
    assert data['tenant'] == os.environ['NACOS_NAMESPACE']
    assert data['group'] == os.environ.get('FAKE_EXPECT_GROUP', os.environ['NACOS_GROUP'])
    assert data['dataId'] == os.environ['NACOS_DATA_ID']
    if publish:
        content = next(arg[len('content@'):] for arg in args if arg.startswith('content@'))
        pathlib.Path(os.environ['FAKE_PUBLISHED']).write_bytes(pathlib.Path(content).read_bytes())
        content = 'true'
    else:
        content = pathlib.Path(os.environ['FAKE_REMOTE']).read_text()
    pathlib.Path(args[args.index('-o')+1]).write_text(content)
    print(os.environ.get('FAKE_HTTP_STATUS', '200'), end='')
PY
chmod 755 "$temp/bin/curl"
export PATH="$temp/bin:$PATH"

run bash "$script" pull --platform android --require-android-signing
check 'local Android signing works without optional auth integrations' test "$status" -eq 0
check 'local default performs no network requests' test ! -e "$FAKE_CURL_LOG"
run bash "$script" pull --platform ios
check 'local iOS sync performs no network requests' test "$status" -eq 0
check 'unnamed env file is ignored' test ! -e "$FAKE_CURL_LOG"
run env BUILD_SECRETS_PROVIDER=nacos bash "$script" pull
check 'requested provider rejects missing coordinates' test "$status" -ne 0
check 'missing coordinates diagnostic names all keys' grep -q 'missing NACOS_SERVER NACOS_NAMESPACE NACOS_GROUP NACOS_DATA_ID' "$temp/output"
run env BUILD_SECRETS_PROVIDER=nacos BUILD_SECRETS_SYNCED=1 bash "$script" pull
check 'sync marker does not bypass configuration validation' test "$status" -ne 0
run env NACOS_CONFIG_FILE="$temp/missing.env" bash "$script" pull
check 'missing named env file fails clearly' test "$status" -ne 0
check 'invalid configuration never contacts server' test ! -e "$FAKE_CURL_LOG"

cp "$app/android/keystore.local.properties" "$temp/signing.properties"
sed '/MYAPP_UPLOAD_KEY_ALIAS=/d' "$temp/signing.properties" >"$app/android/keystore.local.properties"
run bash "$script" pull --platform android --require-android-signing
check 'local signing validates complete properties' test "$status" -ne 0
check 'missing signing key is named' grep -q MYAPP_UPLOAD_KEY_ALIAS "$temp/output"
cp "$temp/signing.properties" "$app/android/keystore.local.properties"

config="$temp/provider.env"
cat >"$config" <<'CONFIG'
BUILD_SECRETS_PROVIDER=nacos
NACOS_SERVER = 'https://config.example.com:8848' # custom server
NACOS_NAMESPACE = 'release-team'
NACOS_GROUP = 'remote-screen-ci'
NACOS_DATA_ID = 'signing.properties'
NACOS_USERNAME = 'ci-user'
NACOS_PASSWORD = 'dummy#password with spaces'
IGNORED=$(touch should-never-exist)
CONFIG
export NACOS_CONFIG_FILE="$config"
cat >"$FAKE_REMOTE" <<'PROPS'
MYAPP_UPLOAD_STORE_FILE=app/release.keystore
MYAPP_UPLOAD_KEY_ALIAS=release
MYAPP_UPLOAD_STORE_PASSWORD=test-store-password
MYAPP_UPLOAD_KEY_PASSWORD=test-key-password
ANDROID_RELEASE_KEYSTORE_BASE64=dGVzdCBrZXlzdG9yZQ==
IOS_ASC_API_KEY_ID=KEYID12345
IOS_ASC_API_ISSUER_ID=00000000-0000-0000-0000-000000000000
IOS_ASC_API_KEY_P8_BASE64=dGVzdCBwOCBrZXk=
PROPS
run env NACOS_GROUP=environment-group FAKE_EXPECT_GROUP=environment-group bash "$script" pull --require-android-signing
check 'custom Nacos coordinates and environment override work' test "$status" -eq 0
check 'provider authenticates with quoted credentials' grep -qx login "$FAKE_CURL_LOG"
check 'provider fetches properties' grep -qx fetch "$FAKE_CURL_LOG"
check 'config is parsed without evaluating shell commands' test ! -e should-never-exist
check 'original signing and ASC property formats are preserved' python3 - "$app" <<'PY'
import pathlib
import stat
import sys
app = pathlib.Path(sys.argv[1])
assert (app / 'android/app/release.keystore').read_text() == 'test keystore'
assert 'MYAPP_UPLOAD_KEY_ALIAS=release' in (app / 'android/keystore.local.properties').read_text()
assert 'ASC_API_KEY_ID=KEYID12345' in (app / 'ios/asc-api-key.properties').read_text()
assert (app / 'AuthKey_KEYID12345.p8').read_text() == 'test p8 key'
for name in ('android/app/release.keystore', 'android/keystore.local.properties', 'ios/asc-api-key.properties', 'AuthKey_KEYID12345.p8'):
    assert stat.S_IMODE((app / name).stat().st_mode) == 0o600
PY
check 'sync output contains no credential or signing values' bash -c '! grep -Eq "dummy#password|test-store-password|test-key-password|test p8 key|test-token" "$1"' _ "$temp/output"

run bash "$script" publish
check 'publish still requires explicit apply' test "$status" -ne 0
run bash "$script" publish --apply
check 'optional provider publishes local properties' test "$status" -eq 0
check 'published properties keep original field names' grep -q '^IOS_ASC_API_KEY_P8_BASE64=dGVzdCBwOCBrZXk=$' "$FAKE_PUBLISHED"
run bash "$script" diff
check 'optional provider compares original properties' test "$status" -eq 0
check 'round trip has no differences' grep -q '与本地一致' "$temp/output"
run env FAKE_CURL_FAILURE=1 bash "$script" pull --platform android --require-android-signing
check 'network failure can use existing local signing' test "$status" -eq 0
run env FAKE_HTTP_STATUS=503 bash "$script" pull --platform android --require-android-signing
check 'HTTP failure can use existing local signing' test "$status" -eq 0
rm "$app/android/app/release.keystore"
run env FAKE_HTTP_STATUS=503 bash "$script" pull --platform android --require-android-signing
check 'remote failure with missing local signing fails' test "$status" -ne 0
rm "$FAKE_CURL_LOG"
run env BUILD_SECRETS_PROVIDER=local bash "$script" pull --platform ios
check 'explicit local provider overrides configured Nacos' test "$status" -eq 0
check 'explicit local provider contacts no server' test ! -e "$FAKE_CURL_LOG"
run env BUILD_SECRETS_PROVIDER=local NACOS_CONFIG_FILE="$temp/missing.env" bash "$script" pull --platform ios
check 'explicit local provider ignores unavailable config files' test "$status" -eq 0
run env NACOS_PASSWORD= bash "$script" pull
check 'partial credentials are rejected' test "$status" -ne 0
check 'partial credentials diagnostic is clear' grep -q 'both NACOS_USERNAME and NACOS_PASSWORD' "$temp/output"

printf 'PASS %d/%d\n' "$passed" "$passed"
