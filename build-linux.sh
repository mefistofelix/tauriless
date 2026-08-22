#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

profile="${1:-debug}"
case "$profile" in
  debug)
    exec cargo build --manifest-path tauriless/Cargo.toml --locked
    ;;
  release)
    exec cargo build --manifest-path tauriless/Cargo.toml --release --locked
    ;;
  *)
    echo "Usage: $0 [debug|release]" >&2
    exit 2
    ;;
esac
