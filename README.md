# TIDAL Rust

An unofficial native Rust/GTK4 TIDAL music player for Linux. Built for a flat,
console-inspired desktop, with Omarchy theme support, live streaming, discovery,
album hearts, listening history and real FFT visualizations. Requires a TIDAL
account and a subscription that permits the requested playback quality.
This project is independent of TIDAL and is not endorsed by TIDAL.

## Install on Omarchy / Arch Linux (x86_64)

Download the `.pkg.tar.zst` from the [latest release](https://github.com/gumuller/tidal-rs/releases/latest),
verify it against the release's `SHA256SUMS`, then install it:

```sh
sudo pacman -U ./tidal-rs-0.6.0-1-x86_64.pkg.tar.zst
```

Pacman installs the required GTK4, mpv, PulseAudio client tools and desktop
integration. Use **TIDAL Rust** in the launcher or `tidal-rs gui`. Authorize in
TIDAL's browser page; passwords are never entered into this client. Omarchy's
PipeWire PulseAudio compatibility service provides audio and spectrum capture.

For an installation without root, the release also provides a native binary
bundle (system libraries are required; this is not an AppImage):

```sh
sudo pacman -S --needed gtk4 mpv libpulse xdg-utils
tar -xzf tidal-rs-0.6.0-linux-x86_64.tar.gz
cd tidal-rs-0.6.0-linux-x86_64
./install.sh
```

The bundle installs to `~/.local/bin` and `~/.local/share`; keep `~/.local/bin`
in your PATH. `./install.sh --uninstall` removes it. Remove a pacman installation
with `sudo pacman -R tidal-rs`. Both methods preserve your login and local data.
These binaries target current Arch/Omarchy x86_64. Other distributions should
build from source with Rust, GTK4 4.12+, a C/C++ toolchain and CMake; binaries
built on rolling Arch may require newer system libraries than those systems ship.

## Optional Omarchy controller

The player works independently of Omarchy. The companion
[omarchy-tidal](https://github.com/gumuller/omarchy-tidal) plugin is a separate,
optional project; install the player first, then:

```sh
omarchy plugin add https://github.com/gumuller/omarchy-tidal.git --enable
```

The plugin provides artwork, play/pause, next/previous, seeking, volume and
show/hide controls from the bar. Hide the player using its header button,
`Ctrl+H`, or the plugin; playback continues. Reopen it from the launcher or
plugin. Closing the window or `Ctrl+Q` exits the app.

The same public session D-Bus controls are available without the plugin:

```sh
tidal-rs control status                  # JSON metadata; no credentials or stream URLs
tidal-rs control show                    # starts the GUI if it is closed
tidal-rs control hide
tidal-rs control toggle-window
tidal-rs control toggle-playback
tidal-rs control next
tidal-rs control previous
tidal-rs control volume 35
tidal-rs control seek 60                  # absolute seconds
tidal-rs control quit
```

Controls use `org.gtk.Actions` on `io.github.tidalrs.Player` on your session bus.
Hiding retains the same window, queue, session and playback worker. Removing the
plugin does not remove or disable the player. This interface is local to your
user session and does not start a network control server.

## Build and packaging

```sh
cargo build --release --locked
./target/release/tidal-rs gui
scripts/package-release.sh               # binary bundle + dependency notices + checksums
cd packaging/arch && makepkg             # source build of the published version
```

GitHub Actions repeats offline tests and packaging in Arch Linux for release
tags. Live account/GUI tests run locally, with isolated state and silent audio.

## Graphical interface

```sh
cargo build
./Play.sh
```

The desktop launcher is **TIDAL Rust**. The interface reuses the saved account
session; new accounts authorize through TIDAL's website. Click an album or
playlist to open its tracks, then click a track to play. Use the bottom player
for pause/resume, previous/next, seeking and volume. `Ctrl+K` focuses catalog
search; suggestions appear as you type (after a short debounce). Select a
suggestion or press Enter to submit. Toggle **Search albums** to find albums by title or
artist; turn it off to find tracks.
Open **Explore** for TIDAL genres, moods, decades, new releases and editorial
playlists. Select a genre, then **New Albums** or **New Tracks** (where TIDAL
provides them) to browse current releases. These lists paginate beyond the
initial editorial previews. Section tiles use current artwork from that
section, cached locally. Use the **heart on an album image** to save or remove
it from your online library. Filled hearts acknowledge albums already saved;
the state is read from your account rather than assumed.
**Play this album** queues the complete album, including albums longer than a
single library page. The listening view places the artwork beside the tracks
and includes a **real audio spectrum analyzer**: 64 logarithmic bands from
24 Hz to 18 kHz, a 4096-point Hann-windowed FFT over 48 kHz PCM, smoothed bars
and peak markers. GTK's frame clock targets 60 fps independently of library
requests. The analyzer monitors only this player's PulseAudio stream via
`parec`; it does not monitor the desktop mix or microphone. `pactl` and `parec`
are required; PipeWire's PulseAudio compatibility server works on Omarchy.

Click the bottom album image to open **the album currently playing**, even
while browsing something else. **Expand player** separately opens artwork,
the queue, controls and a larger visualization. Escape closes it. Choose
**Spectrum**, **Radial spectrum**, **Spectral ribbons** or **Waterfall**; all
use actual FFT frequency data, with the original spectrum as the default.
The UI uses JetBrains Mono
(with a monospace fallback), square surfaces and flat styling.

Volume and streaming quality are stored privately in
`$XDG_STATE_HOME/tidal-rs/preferences.json` (or the default state directory).
They are restored into both the controls and the playback worker. Changing
quality reloads the active stream and restores position, pause and volume.
**High**, **Lossless** and **Max / Hi-Res** are supported. If the service's
LOSSLESS path downgrades to AAC, the client tries its Hi-Res path for FLAC.
The player badge reports delivered quality; its tooltip includes the requested
quality and decoded codec. A downward arrow identifies a lower delivered tier.

**Home** opens at startup with albums from favorite artists (or artists in
favorite tracks when no artists are explicitly followed), TIDAL editorial
selections, recently played songs and recently played albums. Recent listening
is recorded from successful playback in this client and survives restart;
it does not backfill listening from other devices. Each account has a private
`history-USERID.json`, bounded to 200 distinct recent tracks.

Space toggles playback outside the search field, and
`Ctrl+Q` closes the window.

The palette follows Omarchy's current `colors.toml`, including live changes
between light and dark themes. It supports the current
`$XDG_STATE_HOME/omarchy/current/theme` location and the older config location;
no Omarchy configuration edits or hooks are required. `TIDAL_THEME_FILE` can
point to a custom palette. GTK4 supplies native Wayland and fractional scaling.
Album artwork is cached under `$XDG_CACHE_HOME/tidal-rs/artwork`, while library
requests run off the GTK thread. Lists append the next batch automatically near
the bottom of the scroll area; existing items remain in place and duplicate
requests are prevented. Browsing starts with a small batch rather than fetching
the full collection. The CLI retains its numbered pages.

## End-to-end GUI tests

```sh
cargo test
python scripts/gui_e2e.py
```

The GUI tests run against the actual GTK app through AT-SPI and keyboard input,
with live library and stream requests. They require a running Hyprland desktop,
a saved login, Python PyGObject/AT-SPI and Pillow, `wtype`, `grim`, and D-Bus.
They use an isolated copy of the saved session, a private accessibility bus,
and silent audio output. Signing out in the test removes only the copied
session, and palette tests leave the desktop theme unchanged.

Results and screenshots are written to `test-results/gui-e2e-*` (gitignored).
The suite verifies narrow/wide layouts, removed platform subtitle, incremental
loading without page buttons, genre releases, loaded artwork, autocomplete
selection, album saving, real streaming, pause/seek/volume/next, the bottom-art
album navigation, real FFT data and measured spectrum redraw rate, live palettes,
sign-out, clean close, and GTK warnings/panics. It creates a temporary silent
audio sink so PCM monitoring is tested without audible playback, then removes
it. Account checks allow the independently running main app to refresh its
credentials while ensuring the original login remains present and private.
The suite also restarts the app to verify persisted volume, quality and history,
checks already-saved hearts, opens the playing album from the bottom artwork,
and verifies that changing to lossless produces a real FLAC decoder while
preserving pause, position and volume. Offline tests cover private preference
storage, frequency-sensitive rendering for all four modes, and FFT tone/silence
response. Screenshots
are retained for visual review; they are not pixel-perfect baseline tests.

## Terminal interface

```sh
cargo build
./Play.sh online
```

Authorize on TIDAL's website in the browser that opens. The password stays on
TIDAL's website. Favorite tracks appear after login. For example:

```text
play 1
pause
resume
playlists
open 1
play 3
volume 35
quit
```

`tracks 2`, `playlists 2`, and `albums 2` show later pages (50 items per page).
`open N 2` shows the second track page of playlist/album N. Playback continues
through the currently loaded track page; browsing another list cancels that
automatic queue. Stream URLs are resolved when each track starts. `help` lists
all commands. `logout` removes the local saved session and closes the player.

Sessions live in `$XDG_STATE_HOME/tidal-rs/session.json`, or
`~/.local/state/tidal-rs/session.json`, with mode 0600 in a private directory.
Refresh tokens are persisted atomically. The client uses the unofficial v1 API
through `tidalrs`; API changes may break compatibility. Default requested
quality is HIGH; `quality lossless` requests lossless for later tracks, subject
to account/client availability. This backend supports unencrypted BTS and DASH
streams returned for the signed-in user. It does not implement DRM decoding.

Build, twelve offline tests and the local mpv check pass. Browser authorization,
favorite tracks, playlists and saved albums were verified live with the user's
session. An authenticated HIGH-quality track loaded, decoded and advanced in a
silent mpv check. Use `target/debug/tidal-rs online-check` to repeat that check;
it reads the library and decodes one track without audible output.

The download, unpacked artifacts and evidence are in `research/`.
See [the investigation](research/FINDINGS.md) for hashes, code paths, REA
evidence IDs, confidence boundaries, and remaining work. Proprietary artifacts
are excluded from version control.

## Build and inspect

```sh
cargo build --offline
cargo test --offline
cargo run --offline -- --help
printf '%s\n' '{"command":"media.pause"}' | target/debug/tidal-rs translate
printf '%s\n' 'player:{"signal":"media.state","state":"playing"}' | target/debug/tidal-rs signals
```

`--offline` works on this machine with its existing dependency cache; omit it
for the first build on another machine.

## Exercise a Linux playback backend

Run mpv in one terminal (replace the socket path if needed):

```sh
mpv --no-config --idle=yes --no-video --input-ipc-server=/tmp/tidal-rs-mpv.sock
```

Then send commands for a local file you own:

```sh
printf '%s\n' \
  '{"command":"media.load","url":"file:///absolute/path/to/music.flac","streamFormat":"flac"}' \
  '{"command":"media.play"}' \
  | target/debug/tidal-rs send /tmp/tidal-rs-mpv.sock
```

The protocol adapter also accepts HTTPS media URLs. The `online` client obtains
streams separately through the user's authenticated TIDAL session.
`send` waits for mpv command acknowledgements and reports rejected commands. The supported
subset is load, play, pause, stop, seek, volume; unsupported commands fail.

Run the automated backend check without sound or a GUI:

```sh
python scripts/smoke_mpv.py
```
