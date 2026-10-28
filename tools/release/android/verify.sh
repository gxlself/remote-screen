#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/android/verify.sh --apk FILE --aab FILE [--expect-version-code N]
                                       [--expect-version-name S] [--allow-4k-libs] [--json FILE]
                                       [--abis "arm64-v8a x86_64"]
USAGE
}

apk=''
aab=''
expect_version_code=''
expect_version_name=''
allow_4k=false
json_file=''
expect_abis='arm64-v8a'
while (($# > 0)); do
    case "$1" in
        --apk)
            [[ $# -ge 2 && -n "$2" ]] || die '--apk requires a file.'
            apk="$2"
            shift 2
            ;;
        --aab)
            [[ $# -ge 2 && -n "$2" ]] || die '--aab requires a file.'
            aab="$2"
            shift 2
            ;;
        --expect-version-code)
            [[ $# -ge 2 && "$2" =~ ^[0-9]+$ ]] || die '--expect-version-code requires a non-negative integer.'
            expect_version_code="$2"
            shift 2
            ;;
        --expect-version-name)
            [[ $# -ge 2 && -n "$2" ]] || die '--expect-version-name requires a value.'
            expect_version_name="$2"
            shift 2
            ;;
        --allow-4k-libs)
            allow_4k=true
            shift
            ;;
        --abis)
            [[ $# -ge 2 && -n "$2" ]] || die '--abis requires a space-separated list.'
            expect_abis="$2"
            shift 2
            ;;
        --json)
            [[ $# -ge 2 && -n "$2" ]] || die '--json requires a file.'
            json_file="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "Unknown argument: $1"
            ;;
    esac
done

[[ -n "$apk" && -n "$aab" ]] || { usage >&2; die '--apk and --aab are required.'; }
[[ -f "$apk" ]] || die "APK does not exist: $apk"
[[ -f "$aab" ]] || die "AAB does not exist: $aab"

for tool in java keytool jarsigner python3 unzip zipinfo; do
    command -v "$tool" >/dev/null 2>&1 || die "Required command not found: $tool"
done
sdk="$(resolve_sdk)"
build_tools="$(resolve_build_tools "$sdk")"
readelf="$(resolve_readelf "$sdk")"
apksigner="$build_tools/apksigner"
zipalign="$build_tools/zipalign"
aapt2="$build_tools/aapt2"

temp_root="${TMPDIR:-$ANDROID_RELEASE_ROOT/target}"
mkdir -p "$temp_root"
verify_tmp="$(mktemp -d "$temp_root/rs-android-verify.XXXXXX")"
cleanup() { rm -rf -- "$verify_tmp"; }
trap cleanup EXIT

failed=0
pass_check() { printf 'PASS %s\n' "$1"; }
warn_check() { printf 'WARN %s\n' "$1"; }
fail_check() { failed=1; printf 'FAIL %s\n' "$1"; }

normalize_fingerprint() {
    tr -d ':[:space:]\r\n' | tr '[:upper:]' '[:lower:]'
}

apk_signature_output="$verify_tmp/apksigner.txt"
apk_signer_sha256=''
apk_v2=false
apk_v3=false
if "$apksigner" verify --verbose --print-certs "$apk" >"$apk_signature_output" 2>&1; then
    if grep -Eiq 'Verified using v2 scheme[^:]*:[[:space:]]*true' "$apk_signature_output"; then
        apk_v2=true
    fi
    if grep -Eiq 'Verified using v3 scheme[^:]*:[[:space:]]*true' "$apk_signature_output"; then
        apk_v3=true
    fi
    apk_signer_sha256="$(grep -i 'certificate SHA-256 digest:' "$apk_signature_output" | head -n1 | sed 's/.*certificate SHA-256 digest:[[:space:]]*//' | normalize_fingerprint || true)"
fi
if [[ -s "$apk_signature_output" && ( "$apk_v2" == true || "$apk_v3" == true ) && -n "$apk_signer_sha256" ]]; then
    pass_check 'APK signature (v2/v3 and certificate SHA-256)'
else
    fail_check 'APK signature (apksigner verification, v2/v3, or certificate digest failed)'
fi

debug_keystore="$ANDROID_RELEASE_ROOT/app/android/app/debug.keystore"
debug_keytool_output="$verify_tmp/debug-keytool.txt"
debug_signer_sha256=''
if [[ -f "$debug_keystore" ]] && keytool -list -v -keystore "$debug_keystore" \
    -storepass android -alias androiddebugkey >"$debug_keytool_output" 2>&1; then
    debug_signer_sha256="$(grep -iE '^[[:space:]]*SHA-?256:' "$debug_keytool_output" | head -n1 | sed 's/^[^:]*:[[:space:]]*//' | normalize_fingerprint || true)"
fi
if [[ -n "$apk_signer_sha256" && -n "$debug_signer_sha256" && "$apk_signer_sha256" == "$debug_signer_sha256" ]]; then
    fail_check 'APK is signed with the public Android debug certificate'
elif [[ -n "$apk_signer_sha256" && -n "$debug_signer_sha256" ]]; then
    pass_check 'APK is not signed with the Android debug certificate'
else
    fail_check 'Could not inspect the Android debug certificate'
fi

aab_jarsigner_output="$verify_tmp/jarsigner.txt"
aab_keytool_output="$verify_tmp/aab-keytool.txt"
aab_signer_sha256=''
aab_signature_ok=false
if jarsigner -verify "$aab" >"$aab_jarsigner_output" 2>&1; then
    if keytool -printcert -jarfile "$aab" >"$aab_keytool_output" 2>&1; then
        aab_signer_sha256="$(grep -iE '^[[:space:]]*SHA-?256:' "$aab_keytool_output" | head -n1 | sed 's/^[^:]*:[[:space:]]*//' | normalize_fingerprint || true)"
    fi
    if [[ -n "$aab_signer_sha256" && -n "$apk_signer_sha256" && "$aab_signer_sha256" == "$apk_signer_sha256" ]]; then
        aab_signature_ok=true
    fi
fi
if [[ "$aab_signature_ok" == true ]]; then
    pass_check 'AAB signature and APK certificate match'
else
    fail_check 'AAB signature verification or APK/AAB certificate match failed'
fi

apk_badging="$verify_tmp/apk-badging.txt"
apk_package=''
apk_version_code=''
apk_version_name=''
apk_debuggable=false
apk_meta_ok=false
apk_meta_status='FAIL'
if "$aapt2" dump badging "$apk" >"$apk_badging" 2>&1; then
    apk_meta_fields="$(python3 - "$apk_badging" <<'PY'
import re
import sys

text = open(sys.argv[1], encoding="utf-8", errors="replace").read().splitlines()
package = ""
version_code = ""
version_name = ""
for line in text:
    if line.startswith("package:"):
        match = re.search(r"name='([^']*)'", line)
        if match:
            package = match.group(1)
        match = re.search(r"versionCode='([^']*)'", line)
        if match:
            version_code = match.group(1)
        match = re.search(r"versionName='([^']*)'", line)
        if match:
            version_name = match.group(1)
        break
debuggable = any(line.strip() == "application-debuggable" for line in text)
print("\t".join((package, version_code, version_name, "true" if debuggable else "false")))
PY
)"
    IFS=$'\t' read -r apk_package apk_version_code apk_version_name apk_debuggable <<<"$apk_meta_fields"
    apk_meta_ok=true
fi
if [[ "$apk_meta_ok" != true ]]; then
    fail_check 'APK metadata could not be read with aapt2'
elif [[ "$apk_package" != 'ai.gxlself.remotescreen' ]]; then
    fail_check "APK package is ${apk_package:-missing}, expected ai.gxlself.remotescreen"
elif [[ -n "$expect_version_code" && "$apk_version_code" != "$expect_version_code" ]]; then
    fail_check "APK versionCode is ${apk_version_code:-missing}, expected $expect_version_code"
elif [[ -n "$expect_version_name" && "$apk_version_name" != "$expect_version_name" ]]; then
    fail_check "APK versionName is ${apk_version_name:-missing}, expected $expect_version_name"
elif [[ "$apk_debuggable" == true ]]; then
    fail_check 'APK metadata contains application-debuggable'
else
    apk_meta_status='PASS'
    pass_check 'APK metadata (package, version, and non-debuggable)'
fi

aab_meta_status='WARN'
aab_version_code=''
gradle_user_home="${GRADLE_USER_HOME:-$HOME/.gradle}"
bundletool="${RS_BUNDLETOOL:-}"
if [[ -z "$bundletool" ]]; then
    for candidate in \
        "$gradle_user_home"/caches/modules-2/files-2.1/com.android.tools.build/bundletool/*/*/bundletool-*.jar \
        "$gradle_user_home"/caches/*/transforms/*/transformed/instrumented/instrumented-bundletool-*.jar; do
        if [[ -f "$candidate" ]]; then
            bundletool="$candidate"
            break
        fi
    done
fi
if [[ -n "$bundletool" && -f "$bundletool" ]]; then
    bundletool_classpath="$bundletool"
    if [[ -z "${RS_BUNDLETOOL:-}" ]]; then
        while IFS= read -r dependency; do
            bundletool_classpath="$bundletool_classpath:$dependency"
        done < <(find "$gradle_user_home/caches/modules-2/files-2.1" -mindepth 5 -maxdepth 5 -type f -name '*.jar' 2>/dev/null)
        bundletool_java=(java -cp "$bundletool_classpath" com.android.tools.build.bundletool.BundleToolMain)
    else
        bundletool_java=(java -jar "$bundletool")
    fi
    aab_bundletool_output="$verify_tmp/bundletool.txt"
    aab_bundletool_error="$verify_tmp/bundletool.err"
    if "${bundletool_java[@]}" dump manifest --bundle "$aab" \
        --xpath /manifest/@android:versionCode >"$aab_bundletool_output" 2>"$aab_bundletool_error"; then
        aab_version_code="$(grep -Eo '[0-9]+' "$aab_bundletool_output" | tail -n1 || true)"
    fi
    if [[ -n "$aab_version_code" && -n "$apk_version_code" && "$aab_version_code" == "$apk_version_code" \
        && ( -z "$expect_version_code" || "$aab_version_code" == "$expect_version_code" ) ]]; then
        aab_meta_status='PASS'
        pass_check 'AAB metadata versionCode matches APK (bundletool)'
    else
        aab_meta_status='FAIL'
        fail_check "AAB versionCode is ${aab_version_code:-missing}, APK versionCode is ${apk_version_code:-missing}, expected ${expect_version_code:-a value}"
    fi
else
    aab_meta_status='FAIL'
    fail_check 'AAB metadata could not be read: bundletool was not found in the Gradle cache'
fi

if grep -R -nE --include='*.java' --include='*.kt' \
    'TerminalSession[[:space:]]*\(|com\.termux\.terminal\.JNI' \
    "$ANDROID_RELEASE_ROOT/app/android/app/src/main" >"$verify_tmp/termux-source.txt" 2>/dev/null; then
    fail_check 'Termux TerminalSession/JNI source usage would require libtermux.so'
else
    termux_grep_status=$?
    if ((termux_grep_status == 1)); then
        pass_check 'Termux TerminalSession/JNI source usage guard'
    else
        fail_check 'Termux TerminalSession/JNI source usage guard could not inspect the app sources'
    fi
fi

zip_has_member_prefix() {
    local archive="$1"
    local prefix="$2"
    zipinfo -1 "$archive" 2>/dev/null | awk -v prefix="$prefix" 'index($0, prefix) == 1 && length($0) > length(prefix) { found = 1 } END { exit(found ? 0 : 1) }'
}

apk_arm64=false
apk_x86_64=false
aab_arm64=false
aab_x86_64=false
zip_has_member_prefix "$apk" 'lib/arm64-v8a/' && apk_arm64=true || true
zip_has_member_prefix "$apk" 'lib/x86_64/' && apk_x86_64=true || true
zip_has_member_prefix "$aab" 'base/lib/arm64-v8a/' && aab_arm64=true || true
zip_has_member_prefix "$aab" 'base/lib/x86_64/' && aab_x86_64=true || true
abis_ok=true
for abi in $expect_abis; do
    case "$abi" in
        arm64-v8a) [[ "$apk_arm64" == true && "$aab_arm64" == true ]] || abis_ok=false ;;
        x86_64) [[ "$apk_x86_64" == true && "$aab_x86_64" == true ]] || abis_ok=false ;;
        *) die "Unsupported ABI: $abi" ;;
    esac
done
if [[ "$abis_ok" == true ]]; then
    pass_check "64-bit ABI directories in APK and AAB ($expect_abis)"
else
    fail_check "64-bit ABI directories (APK arm64=$apk_arm64 x86_64=$apk_x86_64; AAB arm64=$aab_arm64 x86_64=$aab_x86_64)"
fi

zipalign_output="$verify_tmp/zipalign.txt"
zipalign_ok=false
if "$zipalign" -c -P 16 -v 4 "$apk" >"$zipalign_output" 2>&1; then
    zipalign_ok=true
fi

bad_libraries_file="$verify_tmp/bad-libraries.txt"
: >"$bad_libraries_file"
so_members_file="$verify_tmp/so-members.txt"
if ! zipinfo -1 "$apk" 2>/dev/null | awk '$0 ~ /^lib\/[^/]+\/[^/]+\.so$/ { print }' >"$so_members_file"; then
    : >"$so_members_file"
fi
while IFS= read -r member; do
    [[ -n "$member" ]] || continue
    extracted="$verify_tmp/${member//\//_}"
    if ! unzip -p "$apk" "$member" >"$extracted" 2>/dev/null; then
        printf '%s\n' "$member" >>"$bad_libraries_file"
        continue
    fi
    readelf_output="$verify_tmp/readelf-${#member}.txt"
    if ! "$readelf" -lW "$extracted" >"$readelf_output" 2>&1; then
        printf '%s\n' "$member" >>"$bad_libraries_file"
        continue
    fi
    if ! python3 - "$readelf_output" <<'PY'
import re
import sys

bad = False
load_count = 0
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    if not re.match(r"^\s*LOAD\s", line):
        continue
    load_count += 1
    token = line.split()[-1]
    try:
        if token.lower().startswith("0x"):
            alignment = int(token, 16)
        elif re.fullmatch(r"2\*\*\d+", token):
            alignment = 2 ** int(token.split("**", 1)[1])
        else:
            alignment = int(token, 0)
    except ValueError:
        bad = True
        continue
    if alignment < 0x4000:
        bad = True
if load_count == 0:
    bad = True
sys.exit(1 if bad else 0)
PY
    then
        printf '%s\n' "$member" >>"$bad_libraries_file"
    fi
done <"$so_members_file"

bad_libraries="$(awk 'NF && !seen[$0]++ { print }' "$bad_libraries_file")"
page_status='failed'
if [[ "$zipalign_ok" == true && -z "$bad_libraries" ]]; then
    page_status='ok'
    pass_check '16 KB APK alignment and ELF LOAD segments'
elif [[ "$allow_4k" == true && "$zipalign_ok" == true && -n "$bad_libraries" ]]; then
    page_status='allowed-4k'
    warn_check "16 KB ELF alignment allowed for libraries: $(printf '%s' "$bad_libraries" | tr '\n' ' ')"
else
    fail_check "16 KB alignment (zipalign=$zipalign_ok; non-compliant libraries: ${bad_libraries:-none})"
fi

if [[ -n "$json_file" ]]; then
    mkdir -p "$(dirname "$json_file")"
    python3 - "$json_file" "$apk" "$aab" "$failed" \
        "$apk_signer_sha256" "$apk_v2" "$apk_v3" "$debug_signer_sha256" \
        "$aab_signer_sha256" "$aab_signature_ok" "$apk_package" "$apk_version_code" \
        "$apk_version_name" "$apk_debuggable" "$apk_meta_status" "$aab_meta_status" "$aab_version_code" \
        "$apk_arm64" "$apk_x86_64" "$aab_arm64" "$aab_x86_64" "$zipalign_ok" \
        "$page_status" "$bad_libraries_file" <<'PY'
import json
import pathlib
import sys

def boolean(value):
    return value == "true"

json_path = pathlib.Path(sys.argv[1])
bad_libraries = [line.strip() for line in pathlib.Path(sys.argv[24]).read_text().splitlines() if line.strip()]
payload = {
    "verified": sys.argv[4] == "0",
    "apk": sys.argv[2],
    "aab": sys.argv[3],
    "signerSha256": sys.argv[5] or None,
    "checks": {
        "apkSignature": {
            "status": "PASS" if sys.argv[5] and (boolean(sys.argv[6]) or boolean(sys.argv[7])) else "FAIL",
            "certificateSha256": sys.argv[5] or None,
            "v2": boolean(sys.argv[6]),
            "v3": boolean(sys.argv[7]),
        },
        "debugSignature": {
            "status": "FAIL" if sys.argv[5] and sys.argv[8] and sys.argv[5] == sys.argv[8] else ("PASS" if sys.argv[8] else "FAIL"),
            "certificateSha256": sys.argv[8] or None,
        },
        "aabSignature": {
            "status": "PASS" if boolean(sys.argv[10]) else "FAIL",
            "certificateSha256": sys.argv[9] or None,
            "matchesApk": boolean(sys.argv[10]),
        },
        "apkMetadata": {
            "status": sys.argv[15],
            "package": sys.argv[11] or None,
            "versionCode": int(sys.argv[12]) if sys.argv[12].isdigit() else None,
            "versionName": sys.argv[13],
            "debuggable": boolean(sys.argv[14]),
        },
        "aabMetadata": {
            "status": sys.argv[16],
            "versionCode": int(sys.argv[17]) if sys.argv[17].isdigit() else None,
        },
        "abis": {
            "status": "PASS" if all(boolean(value) for value in sys.argv[18:22]) else "FAIL",
            "apk": [abi for abi, value in (("arm64-v8a", sys.argv[18]), ("x86_64", sys.argv[19])) if boolean(value)],
            "aab": [abi for abi, value in (("arm64-v8a", sys.argv[20]), ("x86_64", sys.argv[21])) if boolean(value)],
        },
        "pageSize16k": {
            "status": "PASS" if sys.argv[23] == "ok" else ("WARN" if sys.argv[23] == "allowed-4k" else "FAIL"),
            "zipalign": boolean(sys.argv[22]),
            "libraries": bad_libraries,
        },
    },
    "abis": [abi for abi, value in (("arm64-v8a", sys.argv[18]), ("x86_64", sys.argv[19])) if boolean(value)],
    "pageSize16k": {"status": sys.argv[23], "libraries": bad_libraries},
}
json_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
PY
fi

if ((failed != 0)); then
    exit 1
fi
