# tidal-rs

An experimental native Rust/GTK4 TIDAL client for Linux and Omarchy, with album
artwork, saved albums, favorite tracks, playlists, catalog search, and online
playback through mpv. It also retains a terminal client and the original player
protocol workbench.

Local Git history starts at `v0.1.0`; discovery and album listening are `v0.2.0`. Genre releases, section artwork
and six visualization modes are `v0.3.0`. The console-style interface, infinite
scrolling, autocomplete and FFT spectrum replace those effects in `v0.4.0`.
This covers the requested discovery and album workflows; full Windows feature
parity remains ongoing.

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
section, cached locally. Open an album and choose **Save album** to add it to your online library.
**Play this album** queues the complete album, including albums longer than a
single library page. The listening view places the artwork beside the tracks
and includes a **real audio spectrum analyzer**: 64 logarithmic bands from
24 Hz to 18 kHz, a 4096-point Hann-windowed FFT over 48 kHz PCM, smoothed bars
and peak markers. GTK's frame clock targets 60 fps independently of library
requests. The analyzer monitors only this player's PulseAudio stream via
`parec`; it does not monitor the desktop mix or microphone. `pactl` and `parec`
are required; PipeWire's PulseAudio compatibility server works on Omarchy.

Click the album image in the bottom bar (or **Expand player**) to open artwork,
the playback queue, transport controls and a larger spectrum. Escape closes it.
The previous generative effects have been removed. The UI uses JetBrains Mono
(with a monospace fallback), square surfaces and flat styling.

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
expanded view, real FFT data and measured spectrum redraw rate, live palettes,
sign-out, clean close, and GTK warnings/panics. It creates a temporary silent
audio sink so PCM monitoring is tested without audible playback, then removes
it. Account checks allow the independently running main app to refresh its
credentials while ensuring the original login remains present and private.
Offline FFT tests distinguish 100 Hz from 4 kHz and verify silence. Screenshots
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

Build, nine offline tests and the local mpv check pass. Browser authorization,
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
