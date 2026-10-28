#!/usr/bin/env bash
# Environment for the desktop integration-test sysroot.

_desktop_env_fail() {
    printf '%s\n' 'RS_DESKTOP_SYSROOT not set or not prepared (prepare the desktop test sysroot first)' >&2
    if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
        exit 2
    fi
    return 2
}

if [[ -z "${RS_DESKTOP_SYSROOT:-}" || ! -d "${RS_DESKTOP_SYSROOT:-}" || ! -f "${RS_DESKTOP_SYSROOT:-}/.rs-sysroot" ]]; then
    _desktop_env_fail
    return 2 2>/dev/null || exit 2
fi

S="$(cd -- "$RS_DESKTOP_SYSROOT" && pwd -P)"
export RS_DESKTOP_SYSROOT="$S"

if TRIPLET="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null)" && [[ -n "$TRIPLET" ]]; then
    :
else
    TRIPLET=x86_64-linux-gnu
fi
export TRIPLET

export PATH="$S/usr/bin:$S/usr/libexec:$PATH"
export LD_LIBRARY_PATH="$S/usr/lib/$TRIPLET:$S/lib/$TRIPLET:$S/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

export PIPEWIRE_MODULE_DIR="$S/usr/lib/$TRIPLET/pipewire-0.3"
export SPA_PLUGIN_DIR="$S/usr/lib/$TRIPLET/spa-0.2"
export PIPEWIRE_CONFIG_DIR="$S/usr/share/pipewire"

if [[ -d "$S/usr/lib/$TRIPLET/wireplumber-0.5" ]]; then
    export WIREPLUMBER_MODULE_DIR="$S/usr/lib/$TRIPLET/wireplumber-0.5"
else
    export WIREPLUMBER_MODULE_DIR="$S/usr/lib/$TRIPLET/wireplumber-0.4"
fi
export WIREPLUMBER_DATA_DIR="$S/usr/share/wireplumber"
export WIREPLUMBER_CONFIG_DIR="$S/usr/share/wireplumber"

export XKB_CONFIG_ROOT="$S/usr/share/X11/xkb"
export XDG_DESKTOP_PORTAL_DIR="$S/usr/share/xdg-desktop-portal/portals"
export XDG_DATA_DIRS="$S/usr/share:${XDG_DATA_DIRS:-/usr/share}"
export XDG_CURRENT_DESKTOP=sway
export RS_DESKTOP_LOG_DIR="${RS_DESKTOP_LOG_DIR:-${TMPDIR:-/tmp}/rs-desktop-logs}"

unset -f _desktop_env_fail
