#!/bin/bash
set -euo pipefail
exec "$(cd "$(dirname "$0")/.." && pwd)/rust/build-ios.sh" "$@"
