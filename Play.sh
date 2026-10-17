#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if (( $# == 0 )); then
  set -- gui
fi
exec ./target/debug/tidal-rs "$@"
