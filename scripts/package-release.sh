#!/usr/bin/env bash
# Build a native Arch x86_64 binary bundle; system GTK/mpv are external dependencies.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
version=$(python -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["package"]["version"])')
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build --release --locked
bundle="dist/tidal-rs-$version-linux-x86_64"
mkdir -p "$bundle/bin"
install -m755 "${CARGO_TARGET_DIR:-target}/release/tidal-rs" "$bundle/bin/tidal-rs"
strip "$bundle/bin/tidal-rs"
cp -R packaging/share "$bundle/"
cp LICENSE README.md THIRD_PARTY.md "$bundle/"
install -m755 packaging/install.sh "$bundle/install.sh"
python scripts/collect-licenses.py "$bundle/licenses"
tar -C dist -czf "$bundle.tar.gz" "$(basename "$bundle")"
(cd dist && sha256sum "$(basename "$bundle").tar.gz" > SHA256SUMS)
printf 'Bundle: %s.tar.gz\n' "$bundle"
