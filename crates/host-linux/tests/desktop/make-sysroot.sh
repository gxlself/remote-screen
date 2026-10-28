#!/usr/bin/env bash
# Build a relocatable desktop-test sysroot from distribution packages.
set -euo pipefail
export LC_ALL=C

usage() {
    printf 'Usage: %s <sysroot-dir> [--dry-run]\n' "$0" >&2
}

if [[ $# -lt 1 || $# -gt 2 ]]; then
    usage
    exit 2
fi

sysroot_arg=$1
dry_run=0
if [[ $# -eq 2 ]]; then
    if [[ $2 != --dry-run ]]; then
        usage
        exit 2
    fi
    dry_run=1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
packages_file="$script_dir/packages.txt"

canonical_path() {
    if command -v realpath >/dev/null 2>&1; then
        realpath -m -- "$1"
        return
    fi
    if command -v readlink >/dev/null 2>&1; then
        readlink -m -- "$1"
        return
    fi
    case $1 in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$PWD" "$1" ;;
    esac
}

sysroot="$(canonical_path "$sysroot_arg")"
home_root="${HOME:-}"
if [[ "$sysroot" == / || "$sysroot" == /usr* || "$sysroot" == /etc* || "$sysroot" == /opt* || "$sysroot" == /var* || ( -n "$home_root" && "$sysroot" == "$home_root"* ) ]]; then
    printf 'sysroot path is not allowed: %s\n' "$sysroot" >&2
    exit 2
fi

mapfile -t requested_packages < <(awk 'NF && $1 !~ /^#/ { print $1 }' "$packages_file")
if [[ ${#requested_packages[@]} -eq 0 ]]; then
    printf 'no packages listed in %s\n' "$packages_file" >&2
    exit 2
fi

dependency_output="$(apt-cache depends \
    --recurse \
    --no-recommends \
    --no-suggests \
    --no-conflicts \
    --no-breaks \
    --no-replaces \
    --no-enhances \
    "${requested_packages[@]}")"

mapfile -t dependency_packages < <(
    printf '%s\n' "$dependency_output" | awk '
        {
            line = $0
            sub(/^[[:space:]]*/, "", line)
            sub(/^\|[[:space:]]*/, "", line)
            sub(/^(Pre-Depends|PreDepends|Depends):[[:space:]]*/, "", line)
            if (line ~ /^<[^>]+>$/ || line == "") {
                next
            }
            if (line ~ /^[A-Za-z0-9][A-Za-z0-9+.-]*$/) {
                print line
            }
        }
    ' | sort -u
)

installed_output="$(dpkg-query -W -f='${Package}\n')"
mapfile -t installed_packages < <(printf '%s\n' "$installed_output" | sed '/^$/d' | sort -u)

mapfile -t packages_to_fetch < <(
    comm -23 \
        <(printf '%s\n' "${dependency_packages[@]}" | sed '/^$/d' | sort -u) \
        <(printf '%s\n' "${installed_packages[@]}" | sed '/^$/d' | sort -u)
)

printf '%s\n' "${packages_to_fetch[@]}" | sed '/^$/d'
printf 'Total: %d packages\n' "${#packages_to_fetch[@]}"

if (( dry_run )); then
    exit 0
fi

debs_dir="$(dirname -- "$sysroot")/.debs"
mkdir -p "$sysroot" "$debs_dir"
if [[ ${#packages_to_fetch[@]} -gt 0 ]]; then
    (
        cd -- "$debs_dir"
        apt-get download "${packages_to_fetch[@]}"
    )
fi

shopt -s nullglob
for deb in "$debs_dir"/*.deb; do
    dpkg-deb -x "$deb" "$sysroot"
done
shopt -u nullglob

{
    printf '# rs-desktop sysroot generated %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '# packages selected for this sysroot\n'
    printf '%s\n' "${packages_to_fetch[@]}"
} > "$sysroot/.rs-sysroot"
printf 'Prepared sysroot: %s\n' "$sysroot"
