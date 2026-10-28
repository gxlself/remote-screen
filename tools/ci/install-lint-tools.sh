#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
ENV_FILE="$ROOT/tools/ci/lint-tools.env"

usage() {
    echo "Usage: $0 [--dir DIR]" >&2
}

install_dir=
while (($#)); do
    case "$1" in
        --dir)
            if (($# < 2)); then
                echo "--dir requires a directory" >&2
                usage
                exit 2
            fi
            install_dir=$2
            shift 2
            ;;
        --dir=*)
            install_dir=${1#*=}
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [[ -z $install_dir ]]; then
    install_dir=${RS_LINT_TOOLS_DIR:-}
fi
if [[ -z $install_dir ]]; then
    install_dir="$ROOT/target/ci-cache/lint-tools/bin"
fi
mkdir -p "$install_dir"
install_dir=$(cd "$install_dir" && pwd -P)

if [[ ! -r $ENV_FILE ]]; then
    echo "lint tool version file not found: $ENV_FILE" >&2
    exit 1
fi
# shellcheck source=/dev/null
source "$ENV_FILE"

if ! command -v go >/dev/null 2>&1; then
    echo "go is not installed" >&2
    exit 2
fi

export GOMODCACHE="${GOMODCACHE:-$ROOT/target/ci-cache/go-mod}"
export GOCACHE="${GOCACHE:-$ROOT/target/ci-cache/go-build}"
export GOPATH="${GOPATH:-$ROOT/target/ci-cache/go}"
mkdir -p "$GOMODCACHE" "$GOCACHE" "$GOPATH"

version_without_v() {
    local value=$1
    printf '%s' "${value#v}"
}

golangci_version() {
    local path=$1
    version_without_v "$("$path" version --short 2>/dev/null)"
}

buf_version() {
    local path=$1
    version_without_v "$("$path" --version 2>/dev/null)"
}

ensure_tool() {
    local name=$1 expected=$2 package=$3 version_command=$4 path version
    path="$install_dir/$name"
    if [[ -x $path ]]; then
        if [[ $version_command == golangci ]]; then
            version=$(golangci_version "$path" || true)
        else
            version=$(buf_version "$path" || true)
        fi
        if [[ $version == "$(version_without_v "$expected")" ]]; then
            echo "ok $name $version $path"
            return 0
        fi
    fi

    echo "installing $name $expected into $install_dir"
    if ! GOBIN="$install_dir" GOTOOLCHAIN=local go install "$package@$expected"; then
        echo "failed to install $name $expected" >&2
        return 1
    fi
    if [[ $version_command == golangci ]]; then
        version=$(golangci_version "$path" || true)
    else
        version=$(buf_version "$path" || true)
    fi
    if [[ $version != "$(version_without_v "$expected")" ]]; then
        echo "$name installed with unexpected version: ${version:-<unknown>} (wanted $(version_without_v "$expected"))" >&2
        return 1
    fi
}

ensure_tool golangci-lint "$GOLANGCI_LINT_VERSION" \
    github.com/golangci/golangci-lint/v2/cmd/golangci-lint golangci
ensure_tool buf "$BUF_VERSION" github.com/bufbuild/buf/cmd/buf buf

echo "golangci-lint $install_dir/golangci-lint ($(golangci_version "$install_dir/golangci-lint"))"
echo "buf $install_dir/buf ($(buf_version "$install_dir/buf"))"
