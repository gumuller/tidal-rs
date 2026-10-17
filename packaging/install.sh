#!/usr/bin/env bash
set -euo pipefail
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
prefix="${TIDAL_INSTALL_PREFIX:-$HOME/.local}"
if [[ ${1:-} == --uninstall ]]; then
  rm -f -- "$prefix/bin/tidal-rs" "$prefix/share/applications/io.github.tidalrs.Player.desktop" "$prefix/share/icons/hicolor/scalable/apps/io.github.tidalrs.Player.svg"
  rm -rf -- "$prefix/share/doc/tidal-rs" "$prefix/share/licenses/tidal-rs"
  echo 'Removed TIDAL Rust. Your login, history and preferences were kept.'
  exit 0
fi
if [[ $# != 0 ]]; then echo 'Usage: ./install.sh [--uninstall]' >&2; exit 2; fi
for program in mpv pactl parec xdg-open; do
  if ! command -v "$program" >/dev/null; then
    echo "Missing $program. On Arch/Omarchy install: sudo pacman -S --needed gtk4 mpv libpulse xdg-utils" >&2
    exit 1
  fi
done
"$source_dir/bin/tidal-rs" --version
install -Dm755 "$source_dir/bin/tidal-rs" "$prefix/bin/tidal-rs"
install -Dm644 "$source_dir/share/applications/io.github.tidalrs.Player.desktop" "$prefix/share/applications/io.github.tidalrs.Player.desktop"
install -Dm644 "$source_dir/share/icons/hicolor/scalable/apps/io.github.tidalrs.Player.svg" "$prefix/share/icons/hicolor/scalable/apps/io.github.tidalrs.Player.svg"
mkdir -p -- "$prefix/share/doc/tidal-rs" "$prefix/share/licenses/tidal-rs"
cp -- "$source_dir/README.md" "$source_dir/THIRD_PARTY.md" "$prefix/share/doc/tidal-rs/"
cp -- "$source_dir/LICENSE" "$prefix/share/licenses/tidal-rs/LICENSE"
cp -R -- "$source_dir/licenses" "$prefix/share/licenses/tidal-rs/dependencies"
if command -v update-desktop-database >/dev/null; then update-desktop-database "$prefix/share/applications"; fi
printf 'Installed TIDAL Rust in %s. Open TIDAL Rust from the launcher, or run: tidal-rs gui\n' "$prefix"
case ":$PATH:" in *":$prefix/bin:"*) ;; *) printf 'Add %s/bin to your PATH for the launcher and optional plugin.\n' "$prefix" ;; esac
