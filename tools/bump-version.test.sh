#!/usr/bin/env bash
# Tests for tools/bump-version.sh. Every test builds a disposable repository
# under $TMPDIR and passes it through --root; the real repository is only read.

set -u

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH= cd -- "$HERE/.." && pwd)
SCRIPT=$HERE/bump-version.sh
WORK=$(mktemp -d "${TMPDIR:-/tmp}/bumpver.XXXXXX")
REPO_PATHS=$WORK/repo.paths
REPO_BEFORE=$WORK/repo.before
TOTAL=0
PASSED=0
FAILED=0
RC=0

cleanup() {
    rm -rf "$WORK"
}
trap cleanup 0

die() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

assert_eq() {
    local label=$1 expected=$2 actual=$3
    [ "$expected" = "$actual" ] || die "$label: expected [$expected], got [$actual]"
}

assert_rc() {
    local label=$1 expected=$2
    assert_eq "$label exit" "$expected" "$RC"
}

assert_contains() {
    local file=$1 needle=$2 label=$3
    grep -F -- "$needle" "$file" >/dev/null || die "$label: missing [$needle] in $file"
}

assert_not_contains() {
    local file=$1 needle=$2 label=$3
    if grep -F -- "$needle" "$file" >/dev/null; then
        die "$label: unexpectedly found [$needle] in $file"
    fi
}

assert_exact_file() {
    local expected=$1 actual=$2 label=$3
    cmp -s "$expected" "$actual" || {
        printf '%s\n' '--- expected ---' >&2
        sed -n '1,80p' "$expected" >&2
        printf '%s\n' '--- actual ---' >&2
        sed -n '1,80p' "$actual" >&2
        die "$label: files differ"
    }
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

file_mtime() {
    stat -c '%Y' "$1" 2>/dev/null || stat -f '%m' "$1"
}

file_mode() {
    stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"
}

snapshot_tree() {
    local dir=$1 output=$2
    (
        cd "$dir" || exit 1
        find . -type f -print | LC_ALL=C sort | while IFS= read -r rel; do
            printf '%s  %s  %s\n' "$rel" "$(sha256_file "$rel")" "$(file_mtime "$rel")"
        done
    ) >"$output"
}

snapshot_hashes() {
    local dir=$1 output=$2
    (
        cd "$dir" || exit 1
        find . -type f -print | LC_ALL=C sort | while IFS= read -r rel; do
            printf '%s  %s\n' "$rel" "$(sha256_file "$rel")"
        done
    ) >"$output"
}

snapshot_selected() {
    local dir=$1 list=$2 output=$3 rel
    : >"$output"
    while IFS= read -r rel; do
        [ -n "$rel" ] || continue
        printf '%s  %s\n' "$rel" "$(sha256_file "$dir/$rel")" >>"$output"
    done <"$list"
}

run_bump() {
    local prefix=$1 root=$2
    shift 2
    "$SCRIPT" --root "$root" "$@" >"$prefix.out" 2>"$prefix.err"
    RC=$?
    return 0
}

replace_literal() {
    local file=$1 old=$2 new=$3
    python3 - "$file" "$old" "$new" <<'PY'
import sys

path, old, new = sys.argv[1:]
with open(path, 'r', encoding='utf-8', newline='') as handle:
    text = handle.read()
if old not in text:
    raise SystemExit('fixture replacement not found: %r' % old)
with open(path, 'w', encoding='utf-8', newline='') as handle:
    handle.write(text.replace(old, new))
PY
}

lock_package_version() {
    local file=$1 package=$2
    python3 - "$file" "$package" <<'PY'
import re
import sys

path, wanted = sys.argv[1:]
with open(path, 'r', encoding='utf-8', newline='') as handle:
    text = handle.read()
for block in re.split(r'(?=^\[\[package\]\])', text, flags=re.M):
    if not block.startswith('[[package]]'):
        continue
    name = re.search(r'^name = "([^"]+)"', block, flags=re.M)
    version = re.search(r'^version = "([^"]+)"', block, flags=re.M)
    if name and version and name.group(1) == wanted:
        print(version.group(1))
        raise SystemExit(0)
raise SystemExit('package not found: %s' % wanted)
PY
}

assert_lock_version() {
    local file=$1 package=$2 expected=$3
    local actual
    actual=$(lock_package_version "$file" "$package") || die "$file: package $package not found"
    assert_eq "$file $package version" "$expected" "$actual"
}

assert_two_occurrences() {
    local file=$1 needle=$2 label=$3 count
    count=$(grep -Fc -- "$needle" "$file" || true)
    assert_eq "$label occurrence count" 2 "$count"
}

make_fixture() {
    local dir=$1 version=$2 major rest minor patch build
    major=${version%%.*}
    rest=${version#*.}
    minor=${rest%%.*}
    patch=${rest#*.}
    build=$((major * 1000000 + minor * 10000 + patch * 100))

    mkdir -p \
        "$dir/crates/a" "$dir/crates/b" \
        "$dir/tools/probe" "$dir/tools/latency/faultnet-proxy" \
        "$dir/fuzz" \
        "$dir/app/ios/RemoteScreen.xcodeproj" "$dir/app/ios/RemoteScreen" \
        "$dir/app/android/app" \
        "$dir/host-ui/src-tauri"

    cat >"$dir/Cargo.toml" <<EOF
[workspace]
resolver = "3"
members = ["crates/*", "tools/probe", "tools/latency/faultnet-proxy"]
exclude = ["host-ui/src-tauri"]

[workspace.package]
version = "$version"
edition = "2021"

[workspace.dependencies]
foo = { version = "0.1.0" }
EOF

    cat >"$dir/Cargo.lock" <<EOF
version = 3

[[package]]
name = "a"
version = "$version"

[[package]]
name = "b"
version = "$version"

[[package]]
name = "foo"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "fixture"
EOF

    cat >"$dir/fuzz/Cargo.lock" <<EOF
version = 3

[[package]]
name = "a"
version = "$version"

[[package]]
name = "fz"
version = "0.1.0"
EOF

    cat >"$dir/crates/a/Cargo.toml" <<'EOF'
[package]
name = "a"
version.workspace = true
edition.workspace = true
EOF

    cat >"$dir/crates/b/Cargo.toml" <<'EOF'
[package]
name = "b"
version.workspace = true
edition.workspace = true
EOF

    cat >"$dir/tools/probe/Cargo.toml" <<'EOF'
[package]
name = "probe"
version.workspace = true
edition.workspace = true
EOF

    cat >"$dir/tools/latency/faultnet-proxy/Cargo.toml" <<'EOF'
[package]
name = "faultnet-proxy"
version.workspace = true
edition.workspace = true
EOF

    cat >"$dir/app/package.json" <<EOF
{
  "name": "fixture-app",
  "version": "$version",
  "private": true,
  "nested": {
    "version": "9.9.9"
  }
}
EOF

    {
        printf '%s\n' '// !$*UTF8*$!' '{'
        printf '\t%s\n' 'objects = {'
        printf '\t\t%s\n' 'Debug = {'
        printf '\t\t\t%s\n' 'buildSettings = {'
        printf '\t\t\t\tCURRENT_PROJECT_VERSION = %s;\n' "$build"
        printf '\t\t\t\tMARKETING_VERSION = %s;\n' "$version"
        printf '\t\t\t%s\n' '};'
        printf '\t\t%s\n' '};'
        printf '\t\t%s\n' 'Release = {'
        printf '\t\t\t%s\n' 'buildSettings = {'
        printf '\t\t\t\tCURRENT_PROJECT_VERSION = %s;\n' "$build"
        printf '\t\t\t\tMARKETING_VERSION = %s;\n' "$version"
        printf '\t\t\t%s\n' '};'
        printf '\t\t%s\n' '};'
        printf '\t%s\n' '};'
        printf '%s\n' '}'
    } >"$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj"

    cat >"$dir/app/ios/RemoteScreen/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>CFBundleShortVersionString</key>
	<string>$(MARKETING_VERSION)</string>
	<key>CFBundleVersion</key>
	<string>$(CURRENT_PROJECT_VERSION)</string>
</dict>
</plist>
EOF

    cat >"$dir/app/android/app/build.gradle" <<'EOF'
android {
    defaultConfig {
        versionCode rsVersionCode
        versionName rsVersionName
    }
}
EOF

    cat >"$dir/host-ui/package.json" <<EOF
{
  "name": "fixture-host-ui",
  "version": "$version",
  "private": true
}
EOF

    cat >"$dir/host-ui/src-tauri/tauri.conf.json" <<EOF
{
  "\$schema": "https://schema.tauri.app/config/2",
  "productName": "Fixture Host",
  "identifier": "example.fixture",
  "version": "$version"
}
EOF

    cat >"$dir/host-ui/src-tauri/Cargo.toml" <<EOF
[package]
name = "remote-host-ui"
version = "$version"
edition = "2021"

[workspace]

[dependencies]
serde = "1"
EOF

    cat >"$dir/host-ui/src-tauri/Cargo.lock" <<EOF
version = 4

[[package]]
name = "remote-host-ui"
version = "$version"
EOF

    cat >"$dir/host-ui/src-tauri/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>LSUIElement</key>
	<true/>
</dict>
</plist>
EOF
}

make_current_mismatch() {
    local dir=$1
    replace_literal "$dir/app/package.json" '"version": "0.1.0"' '"version": "0.0.1"'
    replace_literal "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'MARKETING_VERSION = 0.1.0;' 'MARKETING_VERSION = 1.0;'
    replace_literal "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'CURRENT_PROJECT_VERSION = 10000;' 'CURRENT_PROJECT_VERSION = 1;'
    replace_literal "$dir/host-ui/package.json" '"version": "0.1.0"' '"version": "0.0.0"'
    replace_literal "$dir/host-ui/src-tauri/tauri.conf.json" \
        '"version": "0.1.0"' '"version": "0.0.0"'
    replace_literal "$dir/host-ui/src-tauri/Cargo.toml" \
        'version = "0.1.0"' 'version = "0.0.0"'
    replace_literal "$dir/host-ui/src-tauri/Cargo.lock" \
        'version = "0.1.0"' 'version = "0.0.0"'
}

align_current_fixture() {
    local dir=$1
    replace_literal "$dir/app/package.json" '"version": "0.0.1"' '"version": "0.1.0"'
    replace_literal "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'MARKETING_VERSION = 1.0;' 'MARKETING_VERSION = 0.1.0;'
    replace_literal "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'CURRENT_PROJECT_VERSION = 1;' 'CURRENT_PROJECT_VERSION = 10000;'
    replace_literal "$dir/host-ui/package.json" '"version": "0.0.0"' '"version": "0.1.0"'
    replace_literal "$dir/host-ui/src-tauri/tauri.conf.json" \
        '"version": "0.0.0"' '"version": "0.1.0"'
    replace_literal "$dir/host-ui/src-tauri/Cargo.toml" \
        'version = "0.0.0"' 'version = "0.1.0"'
    replace_literal "$dir/host-ui/src-tauri/Cargo.lock" \
        'version = "0.0.0"' 'version = "0.1.0"'
}

write_current_mismatch_expected() {
    cat >"$1" <<'EOF'
MISMATCH  app/package.json version = 0.0.1 (want 0.1.0)
MISMATCH  app/ios/RemoteScreen.xcodeproj/project.pbxproj CURRENT_PROJECT_VERSION = 1 x2 (want 10000)
MISMATCH  app/ios/RemoteScreen.xcodeproj/project.pbxproj MARKETING_VERSION = 1.0 x2 (want 0.1.0)
MISMATCH  host-ui/package.json version = 0.0.0 (want 0.1.0)
MISMATCH  host-ui/src-tauri/Cargo.lock remote-host-ui = 0.0.0 (want 0.1.0)
MISMATCH  host-ui/src-tauri/Cargo.toml [package] version = 0.0.0 (want 0.1.0)
MISMATCH  host-ui/src-tauri/tauri.conf.json version = 0.0.0 (want 0.1.0)
EOF
    LC_ALL=C sort -o "$1" "$1"
}

assert_mismatch_lines() {
    local prefix=$1 expected=$2 actual="$1.mismatch"
    grep '^MISMATCH' "$prefix.out" >"$actual" || true
    LC_ALL=C sort -o "$actual" "$actual"
    assert_exact_file "$expected" "$actual" 'MISMATCH lines'
}

test_all_consistent() {
    local dir=$WORK/consistent prefix=$WORK/consistent-run
    make_fixture "$dir" 0.1.0
    run_bump "$prefix" "$dir" --check
    assert_rc 'consistent --check' 0
    assert_not_contains "$prefix.out" 'MISMATCH' 'consistent --check'
    assert_contains "$prefix.out" 'Cargo.toml [workspace.package] version = 0.1.0' 'consistent Cargo.toml'
    assert_contains "$prefix.out" 'OK        Cargo.lock 2 workspace packages = 0.1.0' 'consistent Cargo.lock'
    assert_contains "$prefix.out" 'SUMMARY ok=' 'consistent summary'
    assert_contains "$prefix.out" 'mismatch=0' 'consistent summary'
}

test_current_mismatch_is_read_only() {
    local dir=$WORK/mismatch prefix=$WORK/mismatch-run expected=$WORK/mismatch.expected
    make_fixture "$dir" 0.1.0
    make_current_mismatch "$dir"
    write_current_mismatch_expected "$expected"
    snapshot_hashes "$dir" "$prefix.before"
    run_bump "$prefix" "$dir" --check
    assert_rc 'mismatch --check' 1
    assert_mismatch_lines "$prefix" "$expected"
    assert_contains "$prefix.out" 'SUMMARY ok=' 'mismatch summary'
    assert_contains "$prefix.out" 'mismatch=7' 'mismatch summary'
    snapshot_hashes "$dir" "$prefix.after"
    assert_exact_file "$prefix.before" "$prefix.after" '--check must not change files'
}

test_align_current_fixture() {
    local dir=$WORK/align expected=$WORK/align.expected prefix=$WORK/align-run
    make_fixture "$dir" 0.1.0
    make_current_mismatch "$dir"
    cp -R "$dir" "$expected"
    align_current_fixture "$expected"

    run_bump "$prefix" "$dir" 0.1.0
    assert_rc 'align 0.1.0' 0
    run_bump "$prefix.check" "$dir" --check
    assert_rc 'align --check' 0
    assert_not_contains "$prefix.check.out" 'MISMATCH' 'align --check'

    assert_contains "$dir/app/package.json" '"version": "0.1.0"' 'app top-level version'
    assert_contains "$dir/app/package.json" '"version": "9.9.9"' 'app nested version'
    assert_two_occurrences "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'MARKETING_VERSION = 0.1.0;' 'aligned MARKETING_VERSION'
    assert_two_occurrences "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'CURRENT_PROJECT_VERSION = 10000;' 'aligned CURRENT_PROJECT_VERSION'
    assert_lock_version "$dir/Cargo.lock" foo 0.1.0
    assert_contains "$dir/Cargo.lock" 'source = "registry+https://github.com/rust-lang/crates.io-index"' \
        'foo source line'
    assert_lock_version "$dir/fuzz/Cargo.lock" fz 0.1.0
    assert_contains "$dir/Cargo.toml" 'foo = { version = "0.1.0" }' \
        'workspace dependency version'

    diff -r "$expected" "$dir" >"$prefix.diff" 2>&1 || {
        sed -n '1,120p' "$prefix.diff" >&2
        die 'align result differs from expected fixture'
    }
}

test_bump_0_2_3() {
    local dir=$WORK/bump prefix=$WORK/bump-run expected_print=$WORK/bump.print
    make_fixture "$dir" 0.1.0
    python3 - "$dir/app/package.json" <<'PY'
import sys

path = sys.argv[1]
with open(path, 'rb') as handle:
    data = handle.read()
if b'\r\n' in data:
    raise SystemExit('fixture unexpectedly already uses CRLF')
with open(path, 'wb') as handle:
    handle.write(data.replace(b'\n', b'\r\n'))
PY
    chmod 640 "$dir/app/package.json"
    run_bump "$prefix" "$dir" 0.2.3
    assert_rc 'bump 0.2.3' 0
    assert_contains "$dir/Cargo.toml" 'version = "0.2.3"' 'Cargo.toml version'
    assert_contains "$dir/Cargo.toml" 'foo = { version = "0.1.0" }' \
        'workspace dependency version'
    assert_lock_version "$dir/Cargo.lock" a 0.2.3
    assert_lock_version "$dir/Cargo.lock" b 0.2.3
    assert_lock_version "$dir/Cargo.lock" foo 0.1.0
    assert_lock_version "$dir/fuzz/Cargo.lock" a 0.2.3
    assert_lock_version "$dir/fuzz/Cargo.lock" fz 0.1.0
    assert_contains "$dir/app/package.json" '"version": "0.2.3"' 'app version'
    assert_contains "$dir/app/package.json" '"version": "9.9.9"' 'app nested version'
    python3 - "$dir/app/package.json" <<'PY'
import sys

path = sys.argv[1]
with open(path, 'rb') as handle:
    data = handle.read()
if b'\r\n' not in data or data.replace(b'\r\n', b'').count(b'\n') != 0:
    raise SystemExit('CRLF line endings were not preserved')
PY
    assert_eq 'rewritten file mode' 640 "$(file_mode "$dir/app/package.json")"
    assert_contains "$dir/host-ui/package.json" '"version": "0.2.3"' 'host-ui package version'
    assert_contains "$dir/host-ui/src-tauri/tauri.conf.json" '"version": "0.2.3"' 'tauri version'
    assert_contains "$dir/host-ui/src-tauri/Cargo.toml" 'version = "0.2.3"' 'host-ui Cargo version'
    assert_lock_version "$dir/host-ui/src-tauri/Cargo.lock" remote-host-ui 0.2.3
    assert_two_occurrences "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'MARKETING_VERSION = 0.2.3;' 'MARKETING_VERSION 0.2.3'
    assert_two_occurrences "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'CURRENT_PROJECT_VERSION = 20300;' 'CURRENT_PROJECT_VERSION 0.2.3'

    run_bump "$prefix.print" "$dir" --print
    assert_rc '--print' 0
    cat >"$expected_print" <<'EOF'
version=0.2.3
ios_build=20300
android_version_code=20300
EOF
    assert_exact_file "$expected_print" "$prefix.print.out" '--print output'
}

test_downgrade_guard() {
    local dir=$WORK/downgrade prefix=$WORK/downgrade-run
    make_fixture "$dir" 0.1.0
    run_bump "$prefix.up" "$dir" 0.2.3
    assert_rc 'prepare downgrade fixture' 0
    snapshot_tree "$dir" "$prefix.before"

    run_bump "$prefix.refuse" "$dir" 0.2.2
    assert_rc 'downgrade refusal' 1
    assert_contains "$prefix.refuse.err" 'refusing to downgrade 0.2.3 -> 0.2.2 (use --allow-downgrade)' \
        'downgrade refusal message'
    snapshot_tree "$dir" "$prefix.after"
    assert_exact_file "$prefix.before" "$prefix.after" 'refused downgrade must not change files'

    run_bump "$prefix.allow" "$dir" --allow-downgrade 0.2.2
    assert_rc 'allowed downgrade' 0
    assert_contains "$dir/Cargo.toml" 'version = "0.2.2"' 'downgraded Cargo.toml'
    assert_two_occurrences "$dir/app/ios/RemoteScreen.xcodeproj/project.pbxproj" \
        'CURRENT_PROJECT_VERSION = 20200;' 'downgraded CURRENT_PROJECT_VERSION'
}

test_invalid_versions() {
    local dir=$WORK/invalid prefix=$WORK/invalid-run bad
    make_fixture "$dir" 0.1.0
    snapshot_hashes "$dir" "$prefix.before"
    for bad in 1.2 1.2.3.4 01.2.3 1.100.0 1.2.100 2100.0.0 v1.2.3; do
        run_bump "$prefix.$bad" "$dir" "$bad"
        assert_rc "invalid version $bad" 2
        assert_contains "$prefix.$bad.err" 'invalid version' "invalid version $bad message"
    done
    run_bump "$prefix.empty" "$dir" ""
    assert_rc 'empty version' 2
    assert_contains "$prefix.empty.err" 'invalid version' 'empty version message'
    snapshot_hashes "$dir" "$prefix.after"
    assert_exact_file "$prefix.before" "$prefix.after" 'invalid versions must not change files'

    run_bump "$prefix.max" "$dir" 2099.99.99
    assert_rc 'maximum version' 0
    run_bump "$prefix.max.print" "$dir" --print
    assert_rc 'maximum version --print' 0
    assert_contains "$prefix.max.print.out" 'version=2099.99.99' 'maximum version output'
    assert_contains "$prefix.max.print.out" 'ios_build=2099999900' 'maximum iOS build'
    assert_contains "$prefix.max.print.out" 'android_version_code=2099999900' \
        'maximum Android version code'
}

test_missing_file_is_atomic() {
    local dir=$WORK/missing prefix=$WORK/missing-run
    make_fixture "$dir" 0.1.0
    rm "$dir/host-ui/src-tauri/tauri.conf.json"
    snapshot_hashes "$dir" "$prefix.before"
    run_bump "$prefix.set" "$dir" 0.3.0
    assert_rc 'missing tauri.conf.json' 2
    assert_contains "$prefix.set.out" 'MISSING   host-ui/src-tauri/tauri.conf.json' 'missing file message'
    snapshot_hashes "$dir" "$prefix.after"
    assert_exact_file "$prefix.before" "$prefix.after" 'missing file must leave all other files unchanged'
}

test_duplicate_json_version_is_atomic() {
    local dir=$WORK/duplicate prefix=$WORK/duplicate-run
    make_fixture "$dir" 0.1.0
    python3 - "$dir/app/package.json" <<'PY'
import sys

path = sys.argv[1]
with open(path, 'r', encoding='utf-8', newline='') as handle:
    text = handle.read()
needle = '  "version": "0.1.0",\n'
if needle not in text:
    raise SystemExit('fixture top-level version not found')
text = text.replace(needle, needle + '  "version": "9.9.9",\n', 1)
with open(path, 'w', encoding='utf-8', newline='') as handle:
    handle.write(text)
PY
    snapshot_hashes "$dir" "$prefix.before"
    run_bump "$prefix.set" "$dir" 0.3.0
    assert_rc 'duplicate top-level JSON version' 2
    assert_contains "$prefix.set.err" 'expected exactly one two-space-indented version line' \
        'duplicate JSON version message'
    snapshot_hashes "$dir" "$prefix.after"
    assert_exact_file "$prefix.before" "$prefix.after" 'duplicate JSON version must not change files'
}

test_non_inherited_member_is_atomic() {
    local dir=$WORK/noninherit prefix=$WORK/noninherit-run
    make_fixture "$dir" 0.1.0
    replace_literal "$dir/crates/b/Cargo.toml" 'version.workspace = true' 'version = "0.5.0"'
    run_bump "$prefix.check" "$dir" --check
    assert_rc 'non-inherited member --check' 1
    assert_contains "$prefix.check.out" \
        'MISMATCH  crates/b/Cargo.toml version is not workspace-inherited' \
        'non-inherited member mismatch'
    snapshot_hashes "$dir" "$prefix.before"
    run_bump "$prefix.set" "$dir" 0.3.0
    assert_rc 'non-inherited member set' 2
    assert_contains "$prefix.set.err" 'crates/b/Cargo.toml version is not workspace-inherited' \
        'non-inherited member error'
    snapshot_hashes "$dir" "$prefix.after"
    assert_exact_file "$prefix.before" "$prefix.after" 'non-inherited member must not change files'
}

test_same_version_is_noop() {
    local dir=$WORK/idempotent prefix=$WORK/idempotent-run
    make_fixture "$dir" 0.1.0
    make_current_mismatch "$dir"
    run_bump "$prefix.first" "$dir" 0.1.0
    assert_rc 'idempotence first run' 0
    find "$dir" -type f -exec touch -t 200001010000 {} +
    snapshot_tree "$dir" "$prefix.before"
    run_bump "$prefix.second" "$dir" 0.1.0
    assert_rc 'idempotence second run' 0
    snapshot_tree "$dir" "$prefix.after"
    assert_exact_file "$prefix.before" "$prefix.after" 'second run must not rewrite content or mtime'
}

test_missing_optional_locks() {
    local dir=$WORK/optional prefix=$WORK/optional-run
    make_fixture "$dir" 0.1.0
    rm "$dir/fuzz/Cargo.lock"
    run_bump "$prefix" "$dir" --check
    assert_rc 'missing optional locks --check' 0
    assert_contains "$prefix.out" 'SKIP      fuzz/Cargo.lock (missing)' 'fuzz lock skip'
    assert_not_contains "$prefix.out" 'MISMATCH' 'missing optional locks check'
}

write_real_paths() {
    local output=$1 manifest rel
    : >"$output"
    printf '%s\n' \
        'Cargo.toml' \
        'Cargo.lock' \
        'fuzz/Cargo.lock' \
        'app/package.json' \
        'app/ios/RemoteScreen.xcodeproj/project.pbxproj' \
        'app/ios/RemoteScreen/Info.plist' \
        'app/android/app/build.gradle' \
        'host-ui/package.json' \
        'host-ui/src-tauri/tauri.conf.json' \
        'host-ui/src-tauri/Cargo.toml' \
        'host-ui/src-tauri/Cargo.lock' \
        'host-ui/src-tauri/Info.plist' >>"$output"
    for manifest in "$REPO"/crates/*/Cargo.toml; do
        rel=${manifest#"$REPO"/}
        printf '%s\n' "$rel" >>"$output"
    done
    printf '%s\n' \
        'tools/probe/Cargo.toml' \
        'tools/latency/faultnet-proxy/Cargo.toml' >>"$output"
}

copy_real_fixture() {
    local destination=$1 paths=$2 rel
    while IFS= read -r rel; do
        [ -n "$rel" ] || continue
        [ -f "$REPO/$rel" ] || die "real repository is missing $rel"
        mkdir -p "$destination/$(dirname "$rel")"
        cp "$REPO/$rel" "$destination/$rel"
    done <"$paths"
}

test_real_files_smoke() {
    local dir=$WORK/real paths=$REPO_PATHS prefix=$WORK/real-run expected=$WORK/real.expected rel
    command -v git >/dev/null 2>&1 || die 'real-file smoke test requires git'
    write_real_paths "$paths"
    copy_real_fixture "$dir" "$paths"
    snapshot_selected "$REPO" "$paths" "$prefix.source-before"

    # Independent of whether the real repository is currently aligned: align the
    # copy to a version nobody uses, then introduce exactly one mismatch.
    run_bump "$prefix.align" "$dir" 9.8.7
    assert_rc 'real fixture align' 0
    run_bump "$prefix.aligned-check" "$dir" --check
    assert_rc 'real fixture aligned --check' 0
    assert_not_contains "$prefix.aligned-check.out" 'MISMATCH' 'real fixture aligned --check'

    replace_literal "$dir/app/package.json" '"version": "9.8.7"' '"version": "0.0.1"'
    run_bump "$prefix.check" "$dir" --check
    assert_rc 'real fixture --check' 1
    printf '%s\n' 'MISMATCH  app/package.json version = 0.0.1 (want 9.8.7)' >"$expected"
    assert_mismatch_lines "$prefix.check" "$expected"

    snapshot_selected "$REPO" "$paths" "$prefix.source-after"
    assert_exact_file "$prefix.source-before" "$prefix.source-after" \
        'real repository files must not change'
    while IFS= read -r rel; do
        [ -n "$rel" ] || continue
        status=$(cd "$REPO" && git status --short -- "$rel")
        [ -z "$status" ] || die "real repository file changed: $status"
    done <"$paths"
}

run_test() {
    local name=$1 log
    shift
    TOTAL=$((TOTAL + 1))
    log=$WORK/test-$TOTAL.log
    if ("$@" >"$log" 2>&1); then
        PASSED=$((PASSED + 1))
        printf 'ok %d - %s\n' "$TOTAL" "$name"
    else
        FAILED=$((FAILED + 1))
        printf 'not ok %d - %s\n' "$TOTAL" "$name"
        sed -n '1,120p' "$log" >&2
    fi
}

[ -x "$SCRIPT" ] || die "bump-version.sh is not executable"

write_real_paths "$REPO_PATHS"
snapshot_selected "$REPO" "$REPO_PATHS" "$REPO_BEFORE"

run_test 'all files consistent at 0.1.0' test_all_consistent
run_test 'current mismatch is reported without writes' test_current_mismatch_is_read_only
run_test 'current mismatch aligns to 0.1.0' test_align_current_fixture
run_test 'bump to 0.2.3 updates every version location' test_bump_0_2_3
run_test 'downgrade is refused unless allowed' test_downgrade_guard
run_test 'invalid versions are rejected atomically' test_invalid_versions
run_test 'missing required file is rejected atomically' test_missing_file_is_atomic
run_test 'duplicate top-level JSON version is rejected atomically' test_duplicate_json_version_is_atomic
run_test 'non-inherited workspace member is rejected atomically' test_non_inherited_member_is_atomic
run_test 'repeating the same version changes nothing' test_same_version_is_noop
run_test 'missing optional lock files are skipped' test_missing_optional_locks
run_test 'real repository files smoke test' test_real_files_smoke

snapshot_selected "$REPO" "$REPO_PATHS" "$WORK/repo.after"
if ! cmp -s "$REPO_BEFORE" "$WORK/repo.after"; then
    printf '%s\n' 'protected repository files changed:' >&2
    diff -u "$REPO_BEFORE" "$WORK/repo.after" >&2 || true
    exit 1
fi

printf 'PASS %d/%d\n' "$PASSED" "$TOTAL"
if [ "$FAILED" -ne 0 ]; then
    exit 1
fi
