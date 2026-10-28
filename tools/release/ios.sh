#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$script_dir/ios/common.sh"

usage() {
    cat <<'USAGE'
Usage: tools/release/ios.sh [--build N] [--out DIR] [--dry-run] [--upload]
                            [--team-id ID] [--skip-secrets-pull]
                            [--skip-js-install] [--skip-rust] [--skip-pods]
                            [--skip-archive] [--with-tailnet [--allow-no-tailnet]]
                            [--plan]

Signing uses local app/ios/asc-api-key.properties and app/AuthKey_<KEY_ID>.p8.
Set BUILD_SECRETS_PROVIDER=nacos and NACOS_* (or NACOS_CONFIG_FILE) to pull secrets.
--skip-secrets-pull forces local files; --dry-run skips signing.

The App Store build leaves the in-app tailnet out (RS_TAILNET_IOS=off).
--with-tailnet builds and links it; RS_TAILNET_IOS=<path to
RsTailnet.xcframework> links a prebuilt one. --allow-no-tailnet lets a
--with-tailnet release continue when the tailnet build fails.
USAGE
}

arg_error() {
    printf '[ios-release] ERROR: %s\n' "$*" >&2
    exit 2
}

build_number=0
out="$(repo_root)/target/rs-release/ios"
dry_run=false
upload=false
team_arg=""
skip_secrets_pull=false
skip_js_install=false
skip_rust=false
skip_pods=false
skip_archive=false
allow_no_tailnet=false
with_tailnet=false
plan=false

while (($# > 0)); do
    case "$1" in
        --build)
            [[ $# -ge 2 && "$2" =~ ^[0-9]+$ ]] || arg_error '--build requires an integer in 0..99.'
            ((10#$2 <= 99)) || arg_error '--build requires an integer in 0..99.'
            build_number=$((10#$2))
            shift 2
            ;;
        --out)
            [[ $# -ge 2 && -n "$2" ]] || arg_error '--out requires a nonempty directory.'
            out="$2"
            shift 2
            ;;
        --dry-run) dry_run=true; shift ;;
        --upload) upload=true; shift ;;
        --team-id)
            [[ $# -ge 2 && -n "$2" ]] || arg_error '--team-id requires an ID.'
            team_arg="$2"
            shift 2
            ;;
        --skip-secrets-pull) skip_secrets_pull=true; shift ;;
        --skip-js-install) skip_js_install=true; shift ;;
        --skip-rust) skip_rust=true; shift ;;
        --skip-pods) skip_pods=true; shift ;;
        --skip-archive) skip_archive=true; shift ;;
        --with-tailnet) with_tailnet=true; shift ;;
        --allow-no-tailnet) allow_no_tailnet=true; shift ;;
        --plan) plan=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; arg_error "Unknown argument: $1" ;;
    esac
done

if [[ "$dry_run" == true && "$upload" == true ]]; then
    arg_error '--dry-run cannot be combined with --upload.'
fi
# A prebuilt xcframework from the environment opts in like --with-tailnet.
if [[ -n "${RS_TAILNET_IOS:-}" && "$RS_TAILNET_IOS" != off ]]; then
    with_tailnet=true
fi
if [[ "$dry_run" == true ]]; then
    [[ -n "$team_arg" ]] && warn '--team-id is ignored for --dry-run.'
    [[ "$skip_secrets_pull" == true ]] && warn '--skip-secrets-pull is ignored for --dry-run.'
    team_arg=""
    skip_secrets_pull=false
elif [[ -n "$team_arg" ]] && ! valid_team_id "$team_arg"; then
    arg_error '--team-id must match ^[A-Z0-9]{10}$.'
fi

repo="$(repo_root)"
cargo_toml="$repo/Cargo.toml"
read_version_metadata() {
    version="$(read_workspace_version "$cargo_toml")" || die 'Could not read [workspace.package] version from Cargo.toml.'
    [[ -n "$version" ]] || die 'Could not read [workspace.package] version from Cargo.toml.'
    bundle="$(bundle_version "$version" "$build_number")" || die 'Could not calculate CFBundleVersion.'
}

if [[ "$plan" == true ]]; then
    # Plan mode intentionally only reads version metadata and prints commands.
    read_version_metadata
    plan_team='<TEAM_ID>'
    plan_key='<KEY_ID>'
    plan_issuer='<ISSUER_ID>'
    printf '%s\n' "iOS release plan (version=$version bundleVersion=$bundle build=$build_number)"
    printf '%s\n' '1. tool check: xcodebuild xcrun python3 plutil codesign ditto lipo security'
    printf '%s\n' "2. version: MARKETING_VERSION=$version CURRENT_PROJECT_VERSION=$bundle"
    if [[ "$dry_run" == true ]]; then
        printf '%s\n' 'step 3 signing materials: skipped (dry-run)'
    elif [[ "$skip_secrets_pull" == true || "${BUILD_SECRETS_PROVIDER:-}" == local || -z "${BUILD_SECRETS_PROVIDER:-}${NACOS_CONFIG_FILE:-}${NACOS_SERVER:-}${NACOS_NAMESPACE:-}${NACOS_GROUP:-}${NACOS_DATA_ID:-}${NACOS_USERNAME:-}${NACOS_PASSWORD:-}" ]]; then
        printf '%s\n' 'step 3 signing materials: use local app/ios/asc-api-key.properties and app/AuthKey_<KEY_ID>.p8'
    else
        printf '%s\n' 'step 3 signing materials: bash app/scripts/nacos-build-secrets.sh pull --platform ios'
    fi
    [[ "$skip_js_install" == true ]] || printf '%s\n' '4. pnpm --dir app install --frozen-lockfile'
    if [[ "$skip_rust" == true ]]; then
        :
    elif [[ "$with_tailnet" == true ]]; then
        printf '%s\n' '5. (build tailnet if needed, then cd app && CONFIGURATION=Release RS_MOBILE_PROFILE=release ./rust/build-ios.sh)'
    else
        printf '%s\n' '5. cd app && RS_TAILNET_IOS=off CONFIGURATION=Release RS_MOBILE_PROFILE=release ./rust/build-ios.sh'
    fi
    if [[ "$with_tailnet" == true ]]; then
        printf '%s\n' 'tailnet: included (--with-tailnet)'
    else
        printf '%s\n' 'tailnet: not included in this build (App Store default; --with-tailnet to include)'
    fi
    [[ "$skip_pods" == true ]] || printf '%s\n' '6. pod install --project-directory=app/ios'
    if [[ "$skip_archive" == true ]]; then
        printf '%s\n' '7. reuse $out/RemoteScreen.xcarchive'
    elif [[ "$dry_run" == true ]]; then
        printf '%s\n' "7. xcodebuild archive -workspace app/ios/RemoteScreen.xcworkspace -scheme RemoteScreen -configuration Release -destination generic/platform=iOS -archivePath <OUT>/RemoteScreen.xcarchive MARKETING_VERSION=$version CURRENT_PROJECT_VERSION=$bundle CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY=''"
    else
        printf '%s\n' "7. xcodebuild archive -workspace app/ios/RemoteScreen.xcworkspace -scheme RemoteScreen -configuration Release -destination generic/platform=iOS -archivePath <OUT>/RemoteScreen.xcarchive MARKETING_VERSION=$version CURRENT_PROJECT_VERSION=$bundle DEVELOPMENT_TEAM=$plan_team CODE_SIGN_STYLE=Automatic -allowProvisioningUpdates -authenticationKeyPath app/AuthKey_${plan_key}.p8 -authenticationKeyID $plan_key -authenticationKeyIssuerID $plan_issuer"
    fi
    printf '%s\n' '8. verify.sh archive <OUT>/RemoteScreen.xcarchive'
    if [[ "$dry_run" == true ]]; then
        printf '%s\n' '9. ditto -c -k --keepParent <archive>/dSYMs <OUT>/RemoteScreen-<VERSION>-<BUNDLE_VERSION>-dSYMs.zip'
        printf '%s\n' '10. write SHA256SUMS and release-summary.json; dry-run complete'
    else
        printf '%s\n' "9. xcodebuild -exportArchive -archivePath <OUT>/RemoteScreen.xcarchive -exportPath <OUT>/export -exportOptionsPlist <temporary> destination=export teamID=$plan_team"
        printf '%s\n' '10. verify.sh ipa <OUT>/RemoteScreen-<VERSION>-<BUNDLE_VERSION>.ipa --team <TEAM_ID>'
        if [[ "$upload" == true ]]; then
            printf '%s\n' "11. xcodebuild -exportArchive -archivePath <OUT>/RemoteScreen.xcarchive -exportPath <temporary>/upload -exportOptionsPlist <temporary> destination=upload -allowProvisioningUpdates -authenticationKeyPath app/AuthKey_${plan_key}.p8 -authenticationKeyID $plan_key -authenticationKeyIssuerID $plan_issuer"
        else
            printf '%s\n' '11. upload: skipped'
        fi
        printf '%s\n' '12. write dSYM zip, SHA256SUMS and release-summary.json'
    fi
    exit 0
fi

[[ "$(uname -s)" == Darwin ]] || die 'iOS release requires macOS'
if [[ "$dry_run" == false ]]; then
    team="${team_arg:-${RS_IOS_TEAM_ID:-}}"
    [[ -n "$team" ]] || arg_error 'non-dry-run releases require --team-id or RS_IOS_TEAM_ID.'
    valid_team_id "$team" || arg_error '--team-id must match ^[A-Z0-9]{10}$.'
else
    team=""
fi

if [[ "$out" != /* ]]; then out="$PWD/$out"; fi
mkdir -p "$out"
out="$(cd "$out" && pwd -P)"
mkdir -p "$out/logs"
temp_root="${TMPDIR:-$repo/target}"
mkdir -p "$temp_root"
release_tmp="$(mktemp -d "$temp_root/rs-ios-release.XXXXXX")"
cleanup() { rm -rf -- "$release_tmp"; }
trap cleanup EXIT

require_tool() { command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"; }
run_step() {
    local name="$1"; shift
    local started finished elapsed
    started="$(date +%s)"
    log "$name"
    # A caller's `|| die` turns off errexit inside this function: propagate failures explicitly.
    "$@" || return $?
    finished="$(date +%s)"
    elapsed=$((finished - started))
    log "$name completed in ${elapsed}s"
}
run_logged() {
    local name="$1" logfile="$2"; shift 2
    local started finished elapsed status
    started="$(date +%s)"
    log "$name"
    set +e
    "$@" >"$logfile" 2>&1
    status=$?
    set -e
    finished="$(date +%s)"
    elapsed=$((finished - started))
    log "$name completed in ${elapsed}s"
    return "$status"
}

tool_check() {
    local tool
    for tool in xcodebuild xcrun python3 plutil codesign ditto lipo; do require_tool "$tool"; done
    [[ "$dry_run" == true ]] || require_tool security
    [[ "$skip_js_install" == true ]] || { require_tool pnpm; require_tool node; }
    if [[ "$skip_rust" == false ]]; then
        if [[ -x /opt/homebrew/opt/rustup/bin/rustup ]]; then :
        elif command -v rustup >/dev/null 2>&1; then :
        elif [[ -x "${HOME:-}/.cargo/bin/rustup" ]]; then :
        else die 'rustup not found; use --skip-rust or install rustup.'; fi
    fi
    [[ "$skip_pods" == true ]] || [[ -x "${RS_POD:-/opt/homebrew/bin/pod}" ]] || die "Required command not found: ${RS_POD:-/opt/homebrew/bin/pod}"
    xcode="$(xcodebuild -version | head -n1)"
    log "$xcode"
}

run_step '1/12 Tool check' tool_check
read_release_metadata() {
    read_version_metadata
    release_marketing="$(awk '
    /\/\* Release \*\/ = \{/ { in_release=1; next }
    in_release && /MARKETING_VERSION[[:space:]]*=/ {
        sub(/.*=[[:space:]]*/, ""); gsub(/[;[:space:]]/, ""); print; exit
    }
    in_release && /^[[:space:]]*};/ { in_release=0 }
' "$repo/app/ios/RemoteScreen.xcodeproj/project.pbxproj")"
    if [[ -z "$release_marketing" || "$release_marketing" != "$version" ]]; then
        if [[ -n "$release_marketing" ]]; then
            warn "Release MARKETING_VERSION is $release_marketing; run just bump to keep it aligned with Cargo.toml."
        fi
    fi
    git_commit="$(git -C "$repo" rev-parse HEAD 2>/dev/null || printf 'unknown')"
    git_dirty=false
    if [[ -n "$(git -C "$repo" status --porcelain 2>/dev/null)" ]]; then
        git_dirty=true
    fi
    log "version=$version bundleVersion=$bundle commit=$git_commit dirty=$git_dirty"
}
run_step '2/12 Read version and workspace state' read_release_metadata

secrets_source=none
key_id=""
issuer_id=""
p8_path=""
pulled_secrets=false
properties="$repo/app/ios/asc-api-key.properties"
check_signing_material() {
if [[ "$dry_run" == false ]]; then
    if [[ "$skip_secrets_pull" == false ]]; then
        source "$repo/app/scripts/build-secrets-config.sh" || return $?
        load_build_secrets_config || return $?
        if [[ "$BUILD_SECRETS_PROVIDER" == nacos ]]; then
            [[ -f "$repo/app/scripts/nacos-build-secrets.sh" ]] || die 'app/scripts/nacos-build-secrets.sh is missing.'
            (cd "$repo" && bash app/scripts/nacos-build-secrets.sh pull --platform ios) || die 'Could not pull iOS signing materials.'
            secrets_source=nacos
            pulled_secrets=true
        fi
    fi
    if [[ "$pulled_secrets" == false ]]; then secrets_source=local-files; fi
    [[ -f "$properties" ]] || die 'Missing app/ios/asc-api-key.properties.'
    key_id="$(read_prop "$properties" ASC_API_KEY_ID)"
    issuer_id="$(read_prop "$properties" ASC_API_ISSUER_ID)"
    [[ -n "$key_id" ]] || die 'Missing ASC_API_KEY_ID in app/ios/asc-api-key.properties.'
    valid_key_id "$key_id" || die 'ASC_API_KEY_ID has an invalid format.'
    [[ -n "$issuer_id" ]] || die 'Missing ASC_API_ISSUER_ID in app/ios/asc-api-key.properties.'
    valid_issuer_id "$issuer_id" || die 'ASC_API_ISSUER_ID has an invalid format.'
    p8_path="$repo/app/AuthKey_${key_id}.p8"
    [[ -f "$p8_path" ]] || die 'Missing app/AuthKey_<KEY_ID>.p8.'
    [[ ! -L "$p8_path" ]] || die 'app/AuthKey_<KEY_ID>.p8 must be a regular file.'
    p8_mode="$(python3 - "$p8_path" <<'PY'
import os
import stat
import sys
print(oct(stat.S_IMODE(os.stat(sys.argv[1]).st_mode)))
PY
)"
    [[ "$p8_mode" == 0o600 ]] || warn 'AuthKey_<KEY_ID>.p8 permissions are not 600.'
    log 'Found iOS signing properties and AuthKey file.'
    export RS_MASK_TEAM="$team" RS_MASK_KEY="$key_id" RS_MASK_ISSUER="$issuer_id"
    untracked="$(git -C "$repo" status --porcelain --untracked-files=all -- app 2>/dev/null | awk '$1 == "??" { sub(/^\?\?[[:space:]]+/, ""); print }' || true)"
    if [[ "$pulled_secrets" == true && -n "$untracked" ]]; then
        warn 'Nacos pull created untracked app files:'
        printf '%s\n' "$untracked" | mask_stream >&2
    fi
else
    tailnet=unknown
fi
}
run_step '3/12 Check signing material' check_signing_material

if [[ "$skip_js_install" == false ]]; then
    run_logged '4/12 Install JavaScript dependencies' "$out/logs/js-install.log" pnpm --dir "$repo/app" install --frozen-lockfile || die 'JS dependency installation failed.'
fi

bindings_regenerated=false
tailnet="${tailnet:-unknown}"
if [[ "$skip_rust" == false ]]; then
    if [[ "$with_tailnet" == false ]]; then
        # build-ios.sh would otherwise pick up any xcframework left in the tree.
        export RS_TAILNET_IOS=off
    elif [[ -z "${RS_TAILNET_IOS:-}" ]]; then
        tailnet_out="$out/tailnet"
        if run_logged '5/12 Build iOS tailnet' "$out/logs/tailnet.log" env RS_TAILNET_OUT="$tailnet_out" bash "$repo/tailnet/scripts/build-ios.sh"; then
            RS_TAILNET_IOS="$tailnet_out/ios/RsTailnet.xcframework"
            export RS_TAILNET_IOS
        elif [[ "$allow_no_tailnet" == true ]]; then
            tailnet_log_tail="$(tail -n 80 "$out/logs/tailnet.log" || true)"
            [[ -z "$tailnet_log_tail" ]] || printf '%s\n' "$tailnet_log_tail" >&2
            warn 'iOS tailnet build failed; continuing with --allow-no-tailnet.'
            export RS_TAILNET_IOS=off
        else
            tailnet_log_tail="$(tail -n 80 "$out/logs/tailnet.log" || true)"
            [[ -z "$tailnet_log_tail" ]] || printf '%s\n' "$tailnet_log_tail" >&2
            die 'iOS tailnet build failed; use --allow-no-tailnet to publish without tailnet.'
        fi
    fi
    run_logged '5/12 Build Rust and bindings' "$out/logs/rust.log" bash -c "cd '$repo/app' && CONFIGURATION=Release RS_MOBILE_PROFILE=release ./rust/build-ios.sh" || die 'Rust iOS build failed.'
    tailnet="$(tail -n1 "$out/logs/rust.log" | sed -n 's/.*tailnet=//p')"
    [[ -n "$tailnet" ]] || tailnet=unknown
    if [[ -n "$(git -C "$repo" status --porcelain -- app/src/native/generated app/cpp/generated 2>/dev/null)" ]]; then
        bindings_regenerated=true
        warn '仓库里的绑定已过期，本次用重新生成的绑定构建，请提交'
    fi
fi
tailnet_release_gate "$tailnet" "$with_tailnet" "$allow_no_tailnet" || die 'iOS release tailnet gate failed.'

if [[ "$skip_pods" == false ]]; then
    # React Native autolinking runs the CLI from the current directory, so
    # pod install has to start in app/ios, not the repository root.
    (cd "$repo/app/ios" && run_logged '6/12 Install CocoaPods' "$out/logs/pods.log" "${RS_POD:-/opt/homebrew/bin/pod}" install) || die 'CocoaPods installation failed.'
fi

archive="$out/RemoteScreen.xcarchive"
if [[ "$skip_archive" == true ]]; then
    [[ -d "$archive" ]] || die "Archive does not exist for --skip-archive: $archive"
    log '7/12 Reuse archive'
else
    log '7/12 Archive app'
    rm -rf -- "$archive"
    archive_args=(xcodebuild archive -workspace "$repo/app/ios/RemoteScreen.xcworkspace" -scheme RemoteScreen
        -configuration Release -destination 'generic/platform=iOS' -archivePath "$archive"
        -derivedDataPath "$out/DerivedData" "MARKETING_VERSION=$version" "CURRENT_PROJECT_VERSION=$bundle")
    if [[ "$dry_run" == true ]]; then
        archive_args+=(CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="")
    else
        archive_args+=("DEVELOPMENT_TEAM=$team" CODE_SIGN_STYLE=Automatic -allowProvisioningUpdates
            -authenticationKeyPath "$p8_path" -authenticationKeyID "$key_id" -authenticationKeyIssuerID "$issuer_id")
    fi
    set +e
    "${archive_args[@]}" >"$out/logs/archive.log" 2>&1
    archive_status=$?
    set -e
    mask_file "$out/logs/archive.log"
    if ((archive_status != 0)); then
        tail -n 80 "$out/logs/archive.log" | mask_stream >&2
        die 'xcodebuild archive failed.'
    fi
    log '7/12 Archive app completed'
fi

verify_args=("$script_dir/ios/verify.sh" archive "$archive" --version "$version" --bundle-version "$bundle")
[[ "$dry_run" == true ]] && verify_args+=(--unsigned)
run_step '8/12 Verify archive' "${verify_args[@]}" || die 'Archive verification failed.'
archive_app="$archive/Products/Applications/RemoteScreen.app"
python3 "$repo/app/scripts/app-size.py" "$archive_app" >"$out/app-size.json" || die 'Could not measure archived app size.'

dsym_zip="$out/RemoteScreen-${version}-${bundle}-dSYMs.zip"
run_step '12/12 Package dSYMs' ditto -c -k --keepParent "$archive/dSYMs" "$dsym_zip"

ipa=""
uploaded=false
if [[ "$dry_run" == true ]]; then
    :
else
    log '9/12 Export IPA'
    export_plist="$release_tmp/ExportOptions.plist"
    write_export_options "$export_plist" "$team" export || die 'Could not write export options.'
    export_dir="$out/export"
    rm -rf -- "$export_dir"
    mkdir -p "$export_dir"
    export_args=(xcodebuild -exportArchive -archivePath "$archive" -exportPath "$export_dir" -exportOptionsPlist "$export_plist"
        -allowProvisioningUpdates -authenticationKeyPath "$p8_path" -authenticationKeyID "$key_id" -authenticationKeyIssuerID "$issuer_id")
    set +e
    "${export_args[@]}" >"$out/logs/export.log" 2>&1
    export_status=$?
    set -e
    mask_file "$out/logs/export.log"
    if ((export_status != 0)); then
        tail -n 80 "$out/logs/export.log" | mask_stream >&2
        die 'xcodebuild -exportArchive failed.'
    fi
    log '9/12 Export IPA completed'
    source_ipa="$(find "$export_dir" -maxdepth 1 -type f -name '*.ipa' -print -quit)"
    [[ -n "$source_ipa" && -f "$source_ipa" ]] || die 'xcodebuild export did not produce an IPA.'
    ipa="$out/RemoteScreen-${version}-${bundle}.ipa"
    cp "$source_ipa" "$ipa"
    run_step '10/12 Verify IPA' "$script_dir/ios/verify.sh" ipa "$ipa" --version "$version" --bundle-version "$bundle" --team "$team" || die 'IPA verification failed.'
    if [[ "$upload" == true ]]; then
        log '11/12 Upload TestFlight'
        write_export_options "$release_tmp/UploadOptions.plist" "$team" upload || die 'Could not write upload options.'
        upload_dir="$release_tmp/upload"
        mkdir -p "$upload_dir"
        upload_args=(xcodebuild -exportArchive -archivePath "$archive" -exportPath "$upload_dir" -exportOptionsPlist "$release_tmp/UploadOptions.plist"
            -allowProvisioningUpdates -authenticationKeyPath "$p8_path" -authenticationKeyID "$key_id" -authenticationKeyIssuerID "$issuer_id")
        set +e
        "${upload_args[@]}" >"$out/logs/upload.log" 2>&1
        upload_status=$?
        set -e
        mask_file "$out/logs/upload.log"
        if ((upload_status != 0)); then
            tail -n 80 "$out/logs/upload.log" | mask_stream >&2
            die 'TestFlight upload failed; retry with --skip-archive --upload.'
        fi
        uploaded=true
        log '11/12 Upload TestFlight completed'
    fi
fi

log '12/12 Write release manifests'
sum_file="$out/SHA256SUMS"
: >"$sum_file"
printf '%s  %s\n' "$(sha256_of "$dsym_zip")" "$(basename "$dsym_zip")" >>"$sum_file"
if [[ -n "$ipa" ]]; then
    printf '%s  %s\n' "$(sha256_of "$ipa")" "$(basename "$ipa")" >>"$sum_file"
fi

python3 - "$out/app-size.json" "$out/release-summary.json" "$version" "$bundle" "$build_number" "$dry_run" "$uploaded" "$secrets_source" "$tailnet" "$bindings_regenerated" "$git_commit" "$git_dirty" "$xcode" "$dsym_zip" "$ipa" <<'PY'
import datetime
import hashlib
import json
import pathlib
import sys

(size_file, summary_file, version, bundle, build, dry_run, uploaded,
 secrets_source, tailnet, bindings, commit, dirty, xcode, dsym, ipa) = sys.argv[1:]
size = json.loads(pathlib.Path(size_file).read_text())
app_size = size["apps"][0]
artifacts = []
for value in (dsym, ipa):
    if not value:
        continue
    path = pathlib.Path(value)
    artifacts.append({"file": path.name, "bytes": path.stat().st_size,
                      "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
summary = {
    "platform": "ios", "version": version, "bundleVersion": int(bundle),
    "build": int(build), "bundleId": "ai.gxlself.remotescreen",
    "dryRun": dry_run == "true", "signed": dry_run != "true",
    "uploaded": uploaded == "true", "secretsSource": secrets_source,
    # false: this build has no in-app tailnet; null: not rebuilt, so unknown.
    "tailnet": tailnet,
    "tailnetIncluded": None if tailnet == "unknown" else tailnet != "off",
    "bindingsRegenerated": bindings == "true",
    "gitCommit": commit, "gitDirty": dirty == "true", "xcode": xcode,
    "appSizeMiB": app_size["MiB"], "artifacts": artifacts,
    "createdAt": datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z"),
}
pathlib.Path(summary_file).write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
PY
if [[ "$dry_run" == true ]]; then log 'dry-run complete'; else log 'iOS release complete'; fi
