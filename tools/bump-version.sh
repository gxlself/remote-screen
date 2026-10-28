#!/usr/bin/env bash
# Keep every user-visible product version in sync with one source of truth:
#   Cargo.toml -> [workspace.package] version
#
# Updated by this script:
# - Cargo.toml [workspace.package] version (the source of truth)
# - Cargo.lock workspace packages (all package blocks without a source line)
# - fuzz/Cargo.lock, only workspace package names
# - app/package.json top-level version
# - app/ios/RemoteScreen.xcodeproj/project.pbxproj MARKETING_VERSION and
#   CURRENT_PROJECT_VERSION (the latter uses build number 0)
# - host-ui/package.json, host-ui/src-tauri/tauri.conf.json,
#   host-ui/src-tauri/Cargo.toml, and host-ui/src-tauri/Cargo.lock
#
# Checked but never rewritten:
# - every workspace member inherits version.workspace = true
# - app/ios/RemoteScreen/Info.plist references the Xcode build settings
# - app/android/app/build.gradle uses rsVersionCode / rsVersionName
# - host-ui/src-tauri/Info.plist has no CFBundle version keys
#
# Deliberately untouched:
# - app/native/core/package.json: RsCore.podspec reads it, and changing it would
#   invalidate the checksum in app/ios/Podfile.lock.
# - fuzz/Cargo.toml: their own crate versions are
#   independent from the Remote Screen product version.
# - tools/release/win.ps1 reads Cargo.toml itself; tools/release/mac.sh reads
#   tauri.conf.json and only warns when it differs from the workspace version.
# - CHANGELOG.md: the repository does not currently keep one; updating release
#   notes is a manual release step.
#
# All edits are line-oriented and preserve indentation, comments, and newlines.
# The embedded Python is limited to features available in Python 3.9 so this
# also runs with the system Python on macOS. The bash wrapper avoids bash 4+
# features and works with the macOS-provided bash 3.2.

set -u

usage() {
    cat <<'EOF'
Usage:
  tools/bump-version.sh [--root DIR] --check
  tools/bump-version.sh [--root DIR] --print
  tools/bump-version.sh [--root DIR] [--allow-downgrade] X.Y.Z
  tools/bump-version.sh --help

--check              report whether all version locations match Cargo.toml
--print              print the current version and derived mobile build numbers
--root DIR           repository root (default: parent of this script)
--allow-downgrade    permit setting a version lower than the current one

Exit codes: 0 success/all match, 1 version mismatch or downgrade refused,
            2 usage error, invalid version, missing file, or malformed setting.
EOF
}

usage_error() {
    printf 'bump-version: %s\n' "$*" >&2
    printf 'Try tools/bump-version.sh --help for usage.\n' >&2
    exit 2
}

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$script_dir
action=
version=
have_version=0
allow_downgrade=0

while [ "$#" -gt 0 ]; do
    case $1 in
        --root)
            [ "$#" -ge 2 ] || usage_error '--root requires a directory'
            root=$2
            shift 2
            ;;
        --root=*)
            root=${1#--root=}
            shift
            ;;
        --check)
            [ -z "$action" ] || usage_error 'only one of --check, --print, or a version may be used'
            action=check
            shift
            ;;
        --print)
            [ -z "$action" ] || usage_error 'only one of --check, --print, or a version may be used'
            action=print
            shift
            ;;
        --allow-downgrade)
            allow_downgrade=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        --)
            shift
            while [ "$#" -gt 0 ]; do
                [ "$have_version" -eq 0 ] || usage_error 'only one version may be provided'
                version=$1
                have_version=1
                shift
            done
            ;;
        -*)
            usage_error "unknown option: $1"
            ;;
        *)
            [ "$have_version" -eq 0 ] || usage_error 'only one version may be provided'
            version=$1
            have_version=1
            shift
            ;;
    esac
done

if [ -z "$action" ]; then
    [ "$have_version" -eq 1 ] || usage_error 'a version, --check, or --print is required'
    action=set
elif [ "$have_version" -eq 1 ]; then
    usage_error 'a version cannot be combined with --check or --print'
fi

if [ "$action" != set ] && [ "$allow_downgrade" -ne 0 ]; then
    usage_error '--allow-downgrade is only valid when setting a version'
fi

command -v python3 >/dev/null 2>&1 || {
    printf 'bump-version: python3 is required\n' >&2
    exit 2
}

python3 - "$root" "$action" "$version" "$allow_downgrade" <<'PY'
import glob
import os
import re
import stat
import sys
import tempfile


VERSION_RE = re.compile(r'^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$')
JSON_VERSION_RE = re.compile(r'^  "version": "([^"]*)"')
PBX_PATTERNS = {
    'MARKETING_VERSION': re.compile(r'^\t+MARKETING_VERSION = ([^;]+);$'),
    'CURRENT_PROJECT_VERSION': re.compile(r'^\t+CURRENT_PROJECT_VERSION = ([^;]+);$'),
}

REQUIRED_FILES = [
    'Cargo.toml',
    'Cargo.lock',
    'app/package.json',
    'app/ios/RemoteScreen.xcodeproj/project.pbxproj',
    'app/ios/RemoteScreen/Info.plist',
    'app/android/app/build.gradle',
    'host-ui/package.json',
    'host-ui/src-tauri/tauri.conf.json',
    'host-ui/src-tauri/Cargo.toml',
    'host-ui/src-tauri/Cargo.lock',
]

OPTIONAL_LOCKS = [
    'fuzz/Cargo.lock',
]

APP_PLIST = 'app/ios/RemoteScreen/Info.plist'
PBXPROJ = 'app/ios/RemoteScreen.xcodeproj/project.pbxproj'
ANDROID_GRADLE = 'app/android/app/build.gradle'
HOST_UI_PLIST = 'host-ui/src-tauri/Info.plist'
HOST_UI_LOCK = 'host-ui/src-tauri/Cargo.lock'
HOST_UI_PACKAGE = 'host-ui/src-tauri/Cargo.toml'


class Failure(Exception):
    def __init__(self, message, code=2, stdout=False):
        Exception.__init__(self, message)
        self.message = message
        self.code = code
        self.stdout = stdout


def report(status, message):
    print('%-9s %s' % (status, message))


def fail(message):
    raise Failure(message, 2)


def root_path(root, rel):
    return os.path.join(root, *rel.split('/'))


def read_text(root, rel, required=True):
    path = root_path(root, rel)
    if not os.path.isfile(path):
        if required:
            raise Failure('MISSING %s' % rel, 2, stdout=True)
        return None
    try:
        with open(path, 'r', encoding='utf-8', newline='') as handle:
            return handle.read()
    except (OSError, UnicodeError) as exc:
        fail('cannot read %s: %s' % (rel, exc))


def line_parts(line):
    if line.endswith('\r\n'):
        return line[:-2], '\r\n'
    if line.endswith('\n') or line.endswith('\r'):
        return line[:-1], line[-1:]
    return line, ''


def replace_span(line, start, end, value):
    body, ending = line_parts(line)
    return body[:start] + value + body[end:] + ending


def validate_version(value, label='version'):
    match = VERSION_RE.fullmatch(value)
    if not match:
        fail("invalid %s %r: expected X.Y.Z with no leading zeros" % (label, value))
    parts = tuple(int(part) for part in match.groups())
    if parts[0] > 2099:
        fail("invalid %s %r: major version must be <= 2099" % (label, value))
    if parts[1] > 99:
        fail("invalid %s %r: minor version must be <= 99" % (label, value))
    if parts[2] > 99:
        fail("invalid %s %r: patch version must be <= 99" % (label, value))
    return parts


def ios_build_number(parts):
    return parts[0] * 1000000 + parts[1] * 10000 + parts[2] * 100


def section_header_pattern(section):
    return re.compile(r'^\s*\[' + re.escape(section) + r'\]\s*(?:#.*)?$')


def find_section_version(text, section, rel):
    lines = text.splitlines(True)
    header_re = section_header_pattern(section)
    headers = [index for index, line in enumerate(lines)
               if header_re.match(line_parts(line)[0])]
    if len(headers) != 1:
        fail('%s: expected exactly one [%s] section, found %d' %
             (rel, section, len(headers)))
    start = headers[0] + 1
    end = len(lines)
    for index in range(start, len(lines)):
        if line_parts(lines[index])[0].startswith('['):
            end = index
            break
    version_re = re.compile(r'^\s*version\s*=\s*"([^"]*)"')
    matches = []
    for index in range(start, end):
        match = version_re.match(line_parts(lines[index])[0])
        if match:
            matches.append((index, match))
    if len(matches) != 1:
        fail('%s: expected exactly one version line in [%s], found %d' %
             (rel, section, len(matches)))
    index, match = matches[0]
    return {
        'lines': lines,
        'index': index,
        'start': match.start(1),
        'end': match.end(1),
        'value': match.group(1),
    }


def replace_section_version(text, section, value, rel):
    found = find_section_version(text, section, rel)
    lines = found['lines']
    lines[found['index']] = replace_span(
        lines[found['index']], found['start'], found['end'], value)
    return ''.join(lines)


def find_json_version(text, rel):
    lines = text.splitlines(True)
    matches = []
    for index, line in enumerate(lines):
        match = JSON_VERSION_RE.match(line_parts(line)[0])
        if match:
            matches.append((index, match))
    if len(matches) != 1:
        fail('%s: expected exactly one two-space-indented version line, found %d' %
             (rel, len(matches)))
    return lines, matches[0][0], matches[0][1]


def replace_json_version(text, value, rel):
    lines, index, match = find_json_version(text, rel)
    lines[index] = replace_span(lines[index], match.start(1), match.end(1), value)
    return ''.join(lines)


def find_pbx_setting(lines, setting, rel):
    pattern = PBX_PATTERNS[setting]
    matches = []
    for index, line in enumerate(lines):
        match = pattern.match(line_parts(line)[0])
        if match:
            matches.append((index, match))
    if not matches:
        fail('%s: %s was not found' % (rel, setting))
    return matches


def pbx_values(lines, setting, rel):
    return [match.group(1) for _, match in find_pbx_setting(lines, setting, rel)]


def replace_pbx(text, version, build_number, rel):
    lines = text.splitlines(True)
    replacements = []
    replacements.extend((index, match, version)
                        for index, match in find_pbx_setting(lines, 'MARKETING_VERSION', rel))
    replacements.extend((index, match, build_number)
                        for index, match in find_pbx_setting(lines, 'CURRENT_PROJECT_VERSION', rel))
    for index, match, value in replacements:
        lines[index] = replace_span(lines[index], match.start(1), match.end(1), value)
    return ''.join(lines)


def unique_values(values):
    result = []
    for value in values:
        if value not in result:
            result.append(value)
    return result


def pbx_report(rel, setting, values, want):
    shown = ', '.join(unique_values(values))
    message = '%s %s = %s x%d' % (rel, setting, shown, len(values))
    if all(value == want for value in values):
        return 'OK', message
    return 'MISMATCH', '%s (want %s)' % (message, want)


def parse_lock(text, rel):
    lines = text.splitlines(True)
    header_re = re.compile(r'^\s*\[\[package\]\]\s*(?:#.*)?$')
    headers = [index for index, line in enumerate(lines)
               if header_re.match(line_parts(line)[0])]
    if not headers:
        fail('%s: no [[package]] blocks found' % rel)
    name_re = re.compile(r'^\s*name\s*=\s*"([^"]*)"\s*(?:#.*)?$')
    version_re = re.compile(r'^\s*version\s*=\s*"([^"]*)"\s*(?:#.*)?$')
    source_re = re.compile(r'^\s*source\s*=')
    blocks = []
    for position, start in enumerate(headers):
        end = headers[position + 1] if position + 1 < len(headers) else len(lines)
        names = []
        versions = []
        has_source = False
        for index in range(start + 1, end):
            body = line_parts(lines[index])[0]
            name_match = name_re.match(body)
            version_match = version_re.match(body)
            if name_match:
                names.append((index, name_match))
            if version_match:
                versions.append((index, version_match))
            if source_re.match(body):
                has_source = True
        if len(names) != 1:
            fail('%s: package block at line %d has %d name lines' %
                 (rel, start + 1, len(names)))
        if len(versions) != 1:
            fail('%s: package %s has %d version lines' %
                 (rel, names[0][1].group(1), len(versions)))
        blocks.append({
            'start': start,
            'end': end,
            'name': names[0][1].group(1),
            'version': versions[0][1].group(1),
            'version_index': versions[0][0],
            'version_start': versions[0][1].start(1),
            'version_end': versions[0][1].end(1),
            'has_source': has_source,
        })
    return lines, blocks


def lock_targets(blocks, predicate):
    return [block for block in blocks if predicate(block)]


def plural(count, singular, plural_form=None):
    if count == 1:
        return singular
    return plural_form if plural_form is not None else singular + 's'


def lock_report(rel, targets, want, noun):
    bad = [block for block in targets if block['version'] != want]
    values = unique_values([block['version'] for block in targets])
    message = '%s %d %s = %s' % (
        rel, len(targets), plural(len(targets), noun), ', '.join(values))
    if not bad:
        return 'OK', message
    names = ', '.join(block['name'] for block in bad)
    return 'MISMATCH', '%s (want %s; mismatched: %s)' % (message, want, names)


def host_lock_report(rel, target, want):
    message = '%s %s = %s' % (rel, target['name'], target['version'])
    if target['version'] == want:
        return 'OK', message
    return 'MISMATCH', '%s (want %s)' % (message, want)


def replace_lock_versions(lines, targets, value):
    for block in targets:
        index = block['version_index']
        lines[index] = replace_span(
            lines[index], block['version_start'], block['version_end'], value)
    return ''.join(lines)


def member_is_inherited(text):
    lines = text.splitlines(True)
    header_re = section_header_pattern('package')
    headers = [index for index, line in enumerate(lines)
               if header_re.match(line_parts(line)[0])]
    if len(headers) != 1:
        return False
    start = headers[0] + 1
    end = len(lines)
    for index in range(start, len(lines)):
        if line_parts(lines[index])[0].startswith('['):
            end = index
            break
    inherited_re = re.compile(r'^\s*version\.workspace\s*=\s*true\s*(?:#.*)?$')
    declared_re = re.compile(r'^\s*version(?:\.workspace)?\s*=')
    inherited = 0
    declared = 0
    for index in range(start, end):
        body = line_parts(lines[index])[0]
        if declared_re.match(body):
            declared += 1
        if inherited_re.match(body):
            inherited += 1
    return inherited == 1 and declared == 1


def member_paths(root):
    paths = glob.glob(os.path.join(root, 'crates', '*', 'Cargo.toml'))
    for rel in ('tools/probe/Cargo.toml', 'tools/latency/faultnet-proxy/Cargo.toml'):
        path = root_path(root, rel)
        if os.path.isfile(path):
            paths.append(path)
    result = []
    for path in paths:
        rel = os.path.relpath(path, root).replace(os.sep, '/')
        if rel not in result:
            result.append(rel)
    return sorted(result)


def find_plist_refs(text, rel):
    lines = text.splitlines(True)
    refs = {}
    for key in ('CFBundleShortVersionString', 'CFBundleVersion'):
        keys = [index for index, line in enumerate(lines)
                if line_parts(line)[0].strip() == '<key>%s</key>' % key]
        if len(keys) != 1:
            fail('%s: expected exactly one %s key, found %d' % (rel, key, len(keys)))
        index = keys[0] + 1
        if index >= len(lines):
            fail('%s: %s has no string value' % (rel, key))
        match = re.match(r'^\s*<string>(.*)</string>\s*$',
                         line_parts(lines[index])[0])
        if not match:
            fail('%s: %s is not followed by a string value' % (rel, key))
        refs[key] = match.group(1)
    return refs


def validate_android(text, rel):
    lines = [line_parts(line)[0] for line in text.splitlines(True)]
    code_re = re.compile(r'^\s*versionCode\s+rsVersionCode\s*$')
    name_re = re.compile(r'^\s*versionName\s+rsVersionName\s*$')
    code_lines = [line for line in lines if code_re.match(line)]
    name_lines = [line for line in lines if name_re.match(line)]
    if len(code_lines) != 1:
        fail('%s: expected exactly one versionCode rsVersionCode line, found %d' %
             (rel, len(code_lines)))
    if len(name_lines) != 1:
        fail('%s: expected exactly one versionName rsVersionName line, found %d' %
             (rel, len(name_lines)))


def host_plist_has_versions(text):
    return ('CFBundleShortVersionString' in text or 'CFBundleVersion' in text)


def required_missing(root):
    return [rel for rel in REQUIRED_FILES if not os.path.isfile(root_path(root, rel))]


def current_version(root):
    text = read_text(root, 'Cargo.toml')
    found = find_section_version(text, 'workspace.package', 'Cargo.toml')
    parts = validate_version(found['value'], 'Cargo.toml [workspace.package] version')
    return text, found['value'], parts


def root_lock_members(root, root_version):
    text = read_text(root, 'Cargo.lock')
    lines, blocks = parse_lock(text, 'Cargo.lock')
    targets = lock_targets(blocks, lambda block: not block['has_source'])
    if not targets:
        fail('Cargo.lock: no workspace packages without a source line')
    names = [block['name'] for block in targets]
    if len(names) != len(set(names)):
        fail('Cargo.lock: duplicate workspace package names')
    status, message = lock_report('Cargo.lock', targets, root_version, 'workspace package')
    return text, lines, targets, names, status, message


def parse(output):
    if len(sys.argv) != 5:
        fail('internal error: expected four arguments')
    root, action, version, allow_text = sys.argv[1:]
    root = os.path.abspath(root)
    if not os.path.isdir(root):
        fail('root is not a directory: %s' % root)
    try:
        allow_downgrade = int(allow_text)
    except ValueError:
        fail('internal error: invalid allow-downgrade flag')
    return root, action, version, allow_downgrade


def perform_check(root):
    missing = required_missing(root)
    if missing:
        for rel in missing:
            report('MISSING', rel)
        return 2

    cargo_text, current, current_parts = current_version(root)
    ok = 0
    mismatched = 0

    def line(status, message):
        nonlocal ok, mismatched
        report(status, message)
        if status == 'OK':
            ok += 1
        elif status == 'MISMATCH':
            mismatched += 1

    line('OK', 'Cargo.toml [workspace.package] version = %s' % current)

    _, _, root_targets, members, status, message = root_lock_members(root, current)
    line(status, message)

    for rel in OPTIONAL_LOCKS:
        text = read_text(root, rel, required=False)
        if text is None:
            line('SKIP', '%s (missing)' % rel)
            continue
        _, blocks = parse_lock(text, rel)
        targets = lock_targets(
            blocks, lambda block: not block['has_source'] and block['name'] in members)
        status, message = lock_report(rel, targets, current, 'workspace package')
        line(status, message)

    for rel in member_paths(root):
        text = read_text(root, rel)
        if member_is_inherited(text):
            line('OK', '%s version.workspace = true' % rel)
        else:
            line('MISMATCH', '%s version is not workspace-inherited' % rel)

    app_package = read_text(root, 'app/package.json')
    _, _, app_match = find_json_version(app_package, 'app/package.json')
    app_version = app_match.group(1)
    if app_version == current:
        line('OK', 'app/package.json version = %s' % app_version)
    else:
        line('MISMATCH', 'app/package.json version = %s (want %s)' %
             (app_version, current))

    pbx_text = read_text(root, PBXPROJ)
    pbx_lines = pbx_text.splitlines(True)
    line(*pbx_report(PBXPROJ, 'MARKETING_VERSION',
                     pbx_values(pbx_lines, 'MARKETING_VERSION', PBXPROJ), current))
    want_build = str(ios_build_number(current_parts))
    line(*pbx_report(PBXPROJ, 'CURRENT_PROJECT_VERSION',
                     pbx_values(pbx_lines, 'CURRENT_PROJECT_VERSION', PBXPROJ), want_build))

    plist = read_text(root, APP_PLIST)
    refs = find_plist_refs(plist, APP_PLIST)
    expected_refs = {
        'CFBundleShortVersionString': '$(MARKETING_VERSION)',
        'CFBundleVersion': '$(CURRENT_PROJECT_VERSION)',
    }
    if refs == expected_refs:
        line('OK', '%s CFBundleShortVersionString = $(MARKETING_VERSION), '
                   'CFBundleVersion = $(CURRENT_PROJECT_VERSION)' % APP_PLIST)
    else:
        line('MISMATCH', '%s CFBundleShortVersionString = %s, CFBundleVersion = %s '
                         '(want $(MARKETING_VERSION) and $(CURRENT_PROJECT_VERSION))' %
             (APP_PLIST, refs['CFBundleShortVersionString'], refs['CFBundleVersion']))

    android = read_text(root, ANDROID_GRADLE)
    validate_android(android, ANDROID_GRADLE)
    line('OK', '%s versionCode rsVersionCode / versionName rsVersionName' % ANDROID_GRADLE)

    host_package = read_text(root, 'host-ui/package.json')
    _, _, host_package_match = find_json_version(host_package, 'host-ui/package.json')
    host_package_version = host_package_match.group(1)
    if host_package_version == current:
        line('OK', 'host-ui/package.json version = %s' % host_package_version)
    else:
        line('MISMATCH', 'host-ui/package.json version = %s (want %s)' %
             (host_package_version, current))

    tauri = read_text(root, 'host-ui/src-tauri/tauri.conf.json')
    _, _, tauri_match = find_json_version(tauri, 'host-ui/src-tauri/tauri.conf.json')
    tauri_version = tauri_match.group(1)
    if tauri_version == current:
        line('OK', 'host-ui/src-tauri/tauri.conf.json version = %s' % tauri_version)
    else:
        line('MISMATCH', 'host-ui/src-tauri/tauri.conf.json version = %s (want %s)' %
             (tauri_version, current))

    host_cargo = read_text(root, HOST_UI_PACKAGE)
    host_cargo_version = find_section_version(
        host_cargo, 'package', HOST_UI_PACKAGE)['value']
    if host_cargo_version == current:
        line('OK', 'host-ui/src-tauri/Cargo.toml [package] version = %s' % host_cargo_version)
    else:
        line('MISMATCH', 'host-ui/src-tauri/Cargo.toml [package] version = %s (want %s)' %
             (host_cargo_version, current))

    host_lock = read_text(root, HOST_UI_LOCK)
    _, host_blocks = parse_lock(host_lock, HOST_UI_LOCK)
    host_targets = lock_targets(
        host_blocks, lambda block: block['name'] == 'remote-host-ui' and not block['has_source'])
    if len(host_targets) != 1:
        fail('%s: expected exactly one remote-host-ui package without source, found %d' %
             (HOST_UI_LOCK, len(host_targets)))
    line(*host_lock_report(HOST_UI_LOCK, host_targets[0], current))

    host_plist = read_text(root, HOST_UI_PLIST, required=False)
    if host_plist is None:
        line('OK', '%s (missing)' % HOST_UI_PLIST)
    elif host_plist_has_versions(host_plist):
        line('MISMATCH', '%s contains CFBundleShortVersionString or CFBundleVersion' %
             HOST_UI_PLIST)
    else:
        line('OK', '%s no CFBundle version keys' % HOST_UI_PLIST)

    print('SUMMARY ok=%d mismatch=%d' % (ok, mismatched))
    return 1 if mismatched else 0


def stage_changes(root, changes):
    staged = []
    try:
        for rel, text in changes:
            path = root_path(root, rel)
            directory = os.path.dirname(path)
            try:
                original_mode = stat.S_IMODE(os.stat(path).st_mode)
                descriptor, temporary = tempfile.mkstemp(
                    prefix='.bump-version.', dir=directory)
            except OSError as exc:
                fail('cannot create temporary file for %s: %s' % (rel, exc))
            try:
                with os.fdopen(descriptor, 'wb') as handle:
                    handle.write(text.encode('utf-8'))
                    handle.flush()
                    os.fsync(handle.fileno())
                os.chmod(temporary, original_mode)
            except OSError as exc:
                try:
                    os.unlink(temporary)
                except OSError:
                    pass
                fail('cannot stage %s: %s' % (rel, exc))
            staged.append((temporary, path))
    except Exception:
        for temporary, _ in staged:
            try:
                os.unlink(temporary)
            except OSError:
                pass
        raise
    try:
        for temporary, path in staged:
            os.replace(temporary, path)
    except OSError as exc:
        for temporary, _ in staged:
            try:
                os.unlink(temporary)
            except OSError:
                pass
        fail('cannot replace a version file: %s' % exc)


def set_version(root, version, allow_downgrade):
    new_parts = validate_version(version)
    cargo_text, current, current_parts = current_version(root)
    if new_parts < current_parts and not allow_downgrade:
        raise Failure('refusing to downgrade %s -> %s (use --allow-downgrade)' %
                      (current, version), 1)

    missing = required_missing(root)
    if missing:
        for rel in missing:
            report('MISSING', rel)
        return 2

    originals = {}
    for rel in REQUIRED_FILES:
        originals[rel] = read_text(root, rel)

    changed = {}

    def update(rel, text):
        if text != originals[rel]:
            changed[rel] = text

    update('Cargo.toml', replace_section_version(
        cargo_text, 'workspace.package', version, 'Cargo.toml'))

    root_lock_text = originals['Cargo.lock']
    root_lines, root_blocks = parse_lock(root_lock_text, 'Cargo.lock')
    root_targets = lock_targets(root_blocks, lambda block: not block['has_source'])
    if not root_targets:
        fail('Cargo.lock: no workspace packages without a source line')
    root_names = [block['name'] for block in root_targets]
    if len(root_names) != len(set(root_names)):
        fail('Cargo.lock: duplicate workspace package names')
    update('Cargo.lock', replace_lock_versions(root_lines, root_targets, version))

    for rel in OPTIONAL_LOCKS:
        text = read_text(root, rel, required=False)
        if text is None:
            continue
        lines, blocks = parse_lock(text, rel)
        targets = lock_targets(
            blocks, lambda block: not block['has_source'] and block['name'] in root_names)
        new_text = replace_lock_versions(lines, targets, version)
        if new_text != text:
            changed[rel] = new_text

    for rel in member_paths(root):
        text = read_text(root, rel)
        if not member_is_inherited(text):
            fail('%s version is not workspace-inherited' % rel)

    update('app/package.json', replace_json_version(
        originals['app/package.json'], version, 'app/package.json'))
    update(PBXPROJ, replace_pbx(
        originals[PBXPROJ], version, str(ios_build_number(new_parts)), PBXPROJ))

    app_refs = find_plist_refs(originals[APP_PLIST], APP_PLIST)
    if app_refs['CFBundleShortVersionString'] != '$(MARKETING_VERSION)':
        fail('%s CFBundleShortVersionString is not $(MARKETING_VERSION)' % APP_PLIST)
    if app_refs['CFBundleVersion'] != '$(CURRENT_PROJECT_VERSION)':
        fail('%s CFBundleVersion is not $(CURRENT_PROJECT_VERSION)' % APP_PLIST)

    validate_android(originals[ANDROID_GRADLE], ANDROID_GRADLE)

    update('host-ui/package.json', replace_json_version(
        originals['host-ui/package.json'], version, 'host-ui/package.json'))
    update('host-ui/src-tauri/tauri.conf.json', replace_json_version(
        originals['host-ui/src-tauri/tauri.conf.json'], version,
        'host-ui/src-tauri/tauri.conf.json'))
    update(HOST_UI_PACKAGE, replace_section_version(
        originals[HOST_UI_PACKAGE], 'package', version, HOST_UI_PACKAGE))

    host_lines, host_blocks = parse_lock(originals[HOST_UI_LOCK], HOST_UI_LOCK)
    host_targets = lock_targets(
        host_blocks, lambda block: block['name'] == 'remote-host-ui' and not block['has_source'])
    if len(host_targets) != 1:
        fail('%s: expected exactly one remote-host-ui package without source, found %d' %
             (HOST_UI_LOCK, len(host_targets)))
    update(HOST_UI_LOCK, replace_lock_versions(host_lines, host_targets, version))

    host_plist_text = read_text(root, HOST_UI_PLIST, required=False)
    if host_plist_text is not None and host_plist_has_versions(host_plist_text):
        fail('%s contains CFBundleShortVersionString or CFBundleVersion' % HOST_UI_PLIST)

    stage_changes(root, list(changed.items()))
    result = perform_check(root)
    if result != 0:
        fail('post-write check failed', 2)
    return 0


def main():
    root, action, version, allow_downgrade = parse(sys.argv[1:])
    if action == 'check':
        return perform_check(root)
    if action == 'print':
        _, current, parts = current_version(root)
        build = ios_build_number(parts)
        print('version=%s' % current)
        print('ios_build=%d' % build)
        print('android_version_code=%d' % build)
        return 0
    if action == 'set':
        return set_version(root, version, allow_downgrade)
    fail('internal error: unknown action %r' % action)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Failure as failure:
        stream = sys.stdout if failure.stdout else sys.stderr
        print(failure.message, file=stream)
        sys.exit(failure.code)
PY
