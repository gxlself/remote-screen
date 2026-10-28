#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/android/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/android.sh [--build N] [--out DIR] [--prev-version-code N]
                                [--keystore-properties FILE] [--skip-secrets-pull]
                                [--skip-rust] [--skip-js-install] [--skip-build]
                                [--allow-4k-libs] [--with-tailnet [--allow-no-tailnet]]

Signing uses local app/android/keystore.local.properties by default.
Set BUILD_SECRETS_PROVIDER=nacos and NACOS_* (or NACOS_CONFIG_FILE) to pull secrets.
--skip-secrets-pull forces local files.

The release leaves the in-app tailnet out. --with-tailnet builds and packages
it; RS_TAILNET_ANDROID_LIBS=<dir> packages a prebuilt set. --allow-no-tailnet
lets a --with-tailnet release continue when the tailnet build fails.
RS_ANDROID_ABIS (default: arm64-v8a) picks the ABIs; add x86_64 for emulators.
USAGE
}

build_number=0
out="$(repo_root)/target/rs-release/android"
prev_version_code=''
keystore_properties=''
skip_secrets_pull=false
skip_rust=false
skip_js_install=false
skip_build=false
allow_4k=false
allow_no_tailnet=false
with_tailnet=false
while (($# > 0)); do
    case "$1" in
        --build)
            [[ $# -ge 2 && "$2" =~ ^[0-9]+$ ]] || die '--build requires an integer in 0..99.'
            ((10#$2 <= 99)) || die '--build requires an integer in 0..99.'
            build_number=$((10#$2))
            shift 2
            ;;
        --out)
            [[ $# -ge 2 && -n "$2" ]] || die '--out requires a nonempty directory.'
            out="$2"
            shift 2
            ;;
        --prev-version-code)
            [[ $# -ge 2 && "$2" =~ ^[0-9]+$ ]] || die '--prev-version-code requires a non-negative integer.'
            prev_version_code="$2"
            shift 2
            ;;
        --keystore-properties)
            [[ $# -ge 2 && -n "$2" ]] || die '--keystore-properties requires a file.'
            keystore_properties="$2"
            shift 2
            ;;
        --skip-secrets-pull)
            skip_secrets_pull=true
            shift
            ;;
        --skip-rust)
            skip_rust=true
            shift
            ;;
        --skip-js-install)
            skip_js_install=true
            shift
            ;;
        --skip-build)
            skip_build=true
            shift
            ;;
        --allow-4k-libs)
            allow_4k=true
            shift
            ;;
        --with-tailnet)
            with_tailnet=true
            shift
            ;;
        --allow-no-tailnet)
            allow_no_tailnet=true
            shift
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

# A prebuilt set of tailnet libraries from the environment opts in like --with-tailnet.
if [[ -n "${RS_TAILNET_ANDROID_LIBS:-}" ]]; then
    with_tailnet=true
fi

android_abis=()
read -r -a android_abis <<< "${RS_ANDROID_ABIS:-arm64-v8a}"
((${#android_abis[@]} > 0)) || android_abis=(arm64-v8a)
# build-android.sh reads the space-separated list; Gradle wants commas.
export RS_ANDROID_ABIS="${android_abis[*]}"
gradle_abis="$(IFS=,; printf '%s' "${android_abis[*]}")"

if [[ "$out" != /* ]]; then
    out="$PWD/$out"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd -P)"
mkdir -p "$out/logs"
repo="$(repo_root)"
android_dir="$repo/app/android"
gradle="$android_dir/gradlew"
[[ -x "$gradle" ]] || die "Gradle wrapper is not executable: $gradle"
temp_root="${TMPDIR:-$repo/target}"
mkdir -p "$temp_root"
release_tmp="$(mktemp -d "$temp_root/rs-android-release.XXXXXX")"
cleanup() { rm -rf -- "$release_tmp"; }
trap cleanup EXIT

run_step() {
    local name="$1"
    shift
    local started finished elapsed
    started="$(date +%s)"
    log "$name"
    # A caller's `|| die` turns off errexit inside this function: propagate failures explicitly.
    "$@" || return $?
    finished="$(date +%s)"
    elapsed=$((finished - started))
    log "$name completed in ${elapsed}s"
}

require_tool() {
    command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
}

absolute_path() {
    local value="$1"
    if [[ "$value" == /* ]]; then
        printf '%s\n' "$value"
    else
        printf '%s/%s\n' "$PWD" "$value"
    fi
}

missing_signing_keys() {
    local properties_file="$1"
    local key value store_file
    local missing=()
    local keys=(MYAPP_UPLOAD_STORE_FILE MYAPP_UPLOAD_KEY_ALIAS MYAPP_UPLOAD_STORE_PASSWORD MYAPP_UPLOAD_KEY_PASSWORD)
    if [[ ! -f "$properties_file" ]]; then
        printf '%s\n' "${keys[@]}"
        return 0
    fi
    for key in "${keys[@]}"; do
        value="$(awk -v wanted="$key" '
            /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
            {
                split($0, parts, "=")
                name = parts[1]
                gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
                if (name == wanted) {
                    value = substr($0, index($0, "=") + 1)
                    gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
                    print value
                    exit
                }
            }
        ' "$properties_file")"
        [[ -n "$value" ]] || missing+=("$key")
        if [[ "$key" == MYAPP_UPLOAD_STORE_FILE && -n "$value" ]]; then
            store_file="$value"
        fi
    done
    if [[ -n "${store_file:-}" ]]; then
        if [[ "$store_file" != /* ]]; then
            store_file="$android_dir/$store_file"
        fi
        [[ -f "$store_file" ]] || missing+=("MYAPP_UPLOAD_STORE_FILE (file not found)")
    fi
    printf '%s\n' "${missing[@]}"
}

run_version() {
    local log_file="$1"
    shift
    local output status line
    set +e
    output="$(cd "$android_dir" && "$gradle" --no-daemon -q :app:rsPrintVersion "$@" 2>&1)"
    status=$?
    set -e
    printf '%s\n' "$output" >"$log_file"
    if ((status != 0)); then
        cat "$log_file" >&2
        die 'Gradle version query failed.'
    fi
    line="$(printf '%s\n' "$output" | grep '^RS-VERSION ' | tail -n1 || true)"
    [[ -n "$line" ]] || { cat "$log_file" >&2; die 'Gradle did not print an RS-VERSION line.'; }
    VERSION_JSON="${line#RS-VERSION }"
    if ! python3 - "$VERSION_JSON" <<'PY'
import json
import sys
value = json.loads(sys.argv[1])
for key in ("versionName", "versionCode", "buildNumber", "signing", "signingSource"):
    if key not in value:
        raise SystemExit(f"RS-VERSION is missing {key}")
if not isinstance(value["versionName"], str) or not all(isinstance(value[key], int) for key in ("versionCode", "buildNumber")):
    raise SystemExit("RS-VERSION has invalid version types")
PY
    then
        cat "$log_file" >&2
        die 'Gradle returned invalid RS-VERSION JSON.'
    fi
    VERSION_NAME="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["versionName"])' "$VERSION_JSON")"
    VERSION_CODE="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["versionCode"])' "$VERSION_JSON")"
    VERSION_BUILD="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["buildNumber"])' "$VERSION_JSON")"
    SIGNING_STATE="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["signing"])' "$VERSION_JSON")"
}

run_gradle_build() {
    (cd "$android_dir" && "$gradle" "${gradle_build_args[@]}")
}

collect_artifacts() {
    local source_aab="$1" source_apk="$2" output_aab="$3" output_apk="$4"
    [[ -f "$source_aab" ]] || die "Missing AAB: $source_aab"
    [[ -f "$source_apk" ]] || die "Missing APK: $source_apk"
    cp "$source_aab" "$output_aab"
    cp "$source_apk" "$output_apk"
}

tool_check() {
    local tool major
    for tool in java keytool jarsigner python3 node pnpm zipinfo; do
        require_tool "$tool"
    done
    major="$(java -version 2>&1 | sed -nE 's/.*version "([0-9]+).*/\1/p' | head -n1)"
    [[ "$major" =~ ^[0-9]+$ && "$major" -ge 17 ]] || die 'Java 17 or newer is required.'
    if [[ "$skip_rust" != true ]]; then
        require_tool cargo
        cargo ndk --version >/dev/null 2>&1 || die 'cargo-ndk is required unless --skip-rust is used.'
    fi
    ANDROID_RELEASE_SDK="$(resolve_sdk)"
    ANDROID_RELEASE_BUILD_TOOLS="$(resolve_build_tools "$ANDROID_RELEASE_SDK")"
    ANDROID_RELEASE_READELF="$(resolve_readelf "$ANDROID_RELEASE_SDK")"
    export ANDROID_RELEASE_SDK ANDROID_RELEASE_BUILD_TOOLS ANDROID_RELEASE_READELF
}

build_rust_and_tailnet() {
    env RS_MOBILE_PROFILE=release bash "$repo/app/rust/build-android.sh"
    # Link the libraries just built, never a prebuilt set named by the environment.
    export RS_MOBILE_ANDROID_LIBS="${RS_MOBILE_ANDROID_OUT:-$repo/app/native/core/android/src/main/jniLibs}"

    # Without RS_TAILNET_ANDROID_LIBS, Gradle packages no tailnet libraries.
    if [[ "$with_tailnet" == false || -n "${RS_TAILNET_ANDROID_LIBS:-}" ]]; then
        return 0
    fi

    tailnet_out="$release_tmp/tailnet"
    log '5/9 Build Android tailnet'
    if env RS_TAILNET_OUT="$tailnet_out" bash "$repo/tailnet/scripts/build-android.sh" >"$out/logs/tailnet.log" 2>&1 &&
        [[ -d "$tailnet_out/android" ]]; then
        export RS_TAILNET_ANDROID_LIBS="$tailnet_out/android"
        return 0
    fi

    tailnet_log_tail="$(tail -n 80 "$out/logs/tailnet.log" || true)"
    [[ -z "$tailnet_log_tail" ]] || printf '%s\n' "$tailnet_log_tail" >&2
    if [[ "$allow_no_tailnet" == true ]]; then
        warn 'Android tailnet build failed; continuing with --allow-no-tailnet.'
        tailnet_empty="$release_tmp/no-tailnet"
        mkdir -p "$tailnet_empty"
        export RS_TAILNET_ANDROID_LIBS="$tailnet_empty"
    else
        die 'Android tailnet build failed; use --allow-no-tailnet to publish without tailnet.'
    fi
}

run_step '1/9 Tool check' tool_check

ensure_js_gradle_plugin() {
    local plugin_dir="$repo/app/node_modules/@react-native/gradle-plugin"
    [[ -d "$plugin_dir" ]] || die "--skip-js-install requires $plugin_dir; run pnpm --dir app install --frozen-lockfile first."
}

if [[ "$skip_js_install" == true ]]; then
    log '2/9 JavaScript dependency install skipped by --skip-js-install'
    ensure_js_gradle_plugin
else
    run_step '2/9 Install JavaScript dependencies' pnpm --dir "$repo/app" install --frozen-lockfile
fi

version_log="$release_tmp/version.log"
gradle_signing_args=()
if [[ -n "$keystore_properties" ]]; then
    keystore_properties="$(absolute_path "$keystore_properties")"
    gradle_signing_args=("-PrsKeystoreProperties=$keystore_properties")
fi
run_step '3/9 Read version' run_version "$version_log" "-PrsBuildNumber=$build_number" "${gradle_signing_args[@]}"
if [[ -n "$prev_version_code" ]] && ((VERSION_CODE <= 10#$prev_version_code)); then
    die "versionCode $VERSION_CODE must be greater than previous $prev_version_code"
fi

secrets_source='local-files'
if [[ "$keystore_properties" != '' ]]; then
    secrets_source='keystore-properties'
elif [[ "$skip_secrets_pull" != true ]]; then
    source "$repo/app/scripts/build-secrets-config.sh"
    load_build_secrets_config
    if [[ "$BUILD_SECRETS_PROVIDER" == nacos ]]; then
        secrets_script="$repo/app/scripts/nacos-build-secrets.sh"
        [[ -f "$secrets_script" ]] || die 'app/scripts/nacos-build-secrets.sh is missing.'
        secrets_source='nacos'
        bash "$secrets_script" pull --platform android --require-android-signing || die 'Nacos signing-material pull failed.'
    fi
fi

run_step '4/9 Check signing material' run_version "$version_log" "-PrsBuildNumber=$build_number" "${gradle_signing_args[@]}"
if [[ "$SIGNING_STATE" != ready ]]; then
    diagnostic_file="$release_tmp/signing-missing.txt"
    if [[ -n "$keystore_properties" ]]; then
        missing_signing_keys "$keystore_properties" >"$diagnostic_file"
    elif [[ -f "$android_dir/keystore.local.properties" ]]; then
        missing_signing_keys "$android_dir/keystore.local.properties" >"$diagnostic_file"
    elif [[ -f "$android_dir/keystore.properties" ]]; then
        missing_signing_keys "$android_dir/keystore.properties" >"$diagnostic_file"
    else
        printf '%s\n' MYAPP_UPLOAD_STORE_FILE MYAPP_UPLOAD_KEY_ALIAS MYAPP_UPLOAD_STORE_PASSWORD MYAPP_UPLOAD_KEY_PASSWORD >"$diagnostic_file"
    fi
    missing_keys="$(paste -sd ',' "$diagnostic_file")"
    die "Release signing is not ready; missing: $missing_keys"
fi

if [[ "$skip_rust" == true ]]; then
    log '5/9 Rust build skipped by --skip-rust'
else
    run_step '5/9 Build Rust (and tailnet) Android libraries' build_rust_and_tailnet
fi

if [[ "$skip_build" == true ]]; then
    log '6/9 Gradle build skipped by --skip-build'
else
    gradle_build_args=(--no-daemon :app:bundleRelease :app:assembleRelease "-PrsBuildNumber=$build_number" "-PreactNativeArchitectures=$gradle_abis")
    if [[ -n "$keystore_properties" ]]; then
        gradle_build_args+=("-PrsKeystoreProperties=$keystore_properties")
    fi
    run_step '6/9 Build AAB and APK' run_gradle_build
fi

bundle_source="$android_dir/app/build/outputs/bundle/release/app-release.aab"
apk_source="$android_dir/app/build/outputs/apk/release/app-release.apk"
unsigned_apk="$android_dir/app/build/outputs/apk/release/app-release-unsigned.apk"
[[ ! -e "$unsigned_apk" ]] || die "Unsigned APK was produced: $unsigned_apk"
bundle_output="$out/RemoteScreen-${VERSION_NAME}-${VERSION_CODE}.aab"
apk_output="$out/RemoteScreen-${VERSION_NAME}-${VERSION_CODE}.apk"
run_step '7/9 Collect release artifacts' collect_artifacts "$bundle_source" "$apk_source" "$bundle_output" "$apk_output"

verify_args=(--apk "$apk_output" --aab "$bundle_output" --abis "${android_abis[*]}" --expect-version-code "$VERSION_CODE" --expect-version-name "$VERSION_NAME" --json "$out/verify.json")
[[ "$allow_4k" == true ]] && verify_args+=(--allow-4k-libs)
run_step '8/9 Verify release artifacts' "$script_dir/android/verify.sh" "${verify_args[@]}"

apk_members="$release_tmp/apk-members.txt"
zipinfo -1 "$apk_output" >"$apk_members" || die 'Could not list APK members for tailnet verification.'
tailnet="$(tailnet_state_from_members "$apk_members" "${android_abis[@]}")" || die 'Could not determine APK tailnet state.'
tailnet_release_gate "$tailnet" "$with_tailnet" "$allow_no_tailnet" || die 'Android release tailnet gate failed.'

run_step '9/9 Write release manifests' python3 - "$out" "$apk_output" "$bundle_output" "$VERSION_NAME" "$VERSION_CODE" "$VERSION_BUILD" "$secrets_source" "$repo" "$tailnet" <<'PY'
import hashlib
import json
import pathlib
import subprocess
import sys

out = pathlib.Path(sys.argv[1])
apk = pathlib.Path(sys.argv[2])
aab = pathlib.Path(sys.argv[3])
version_name = sys.argv[4]
version_code = int(sys.argv[5])
build_number = int(sys.argv[6])
secrets_source = sys.argv[7]
repo = pathlib.Path(sys.argv[8])
tailnet = sys.argv[9]
verify_path = out / "verify.json"
verify = json.loads(verify_path.read_text(encoding="utf-8"))

def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()

files = []
for path in (aab, apk):
    files.append({"name": path.name, "bytes": path.stat().st_size, "sha256": sha256(path)})
(out / "SHA256SUMS").write_text("".join(f"{item['sha256']}  {item['name']}\n" for item in files), encoding="utf-8")
try:
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True, stderr=subprocess.DEVNULL).strip()
except (OSError, subprocess.CalledProcessError):
    commit = None
page = verify.get("pageSize16k", {})
if isinstance(page, str):
    page = {"status": page, "libraries": []}
summary = {
    "versionName": version_name,
    "versionCode": version_code,
    "buildNumber": build_number,
    "files": files,
    "signerSha256": verify.get("signerSha256"),
    "abis": verify.get("abis", []),
    "pageSize16k": page,
    "tailnet": tailnet,
    "gitCommit": commit,
    "secretsSource": secrets_source,
}
(out / "release-summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
PY

log "AAB: $bundle_output"
log "APK: $apk_output"
log "Release ${VERSION_NAME} (${VERSION_CODE}, build ${VERSION_BUILD}) verified; signer=${out}/verify.json"
