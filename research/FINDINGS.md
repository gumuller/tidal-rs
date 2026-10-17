# TIDAL Desktop investigation

## Native interface update — 2026-10-17

A GTK4 interface now shares the authenticated API and mpv playback core with
the terminal client. `./Play.sh` starts it; `./Play.sh online` retains the CLI.
The interface reads and live-reloads Omarchy's palette without modifying desktop
configuration. Library requests and playback run in a background worker, and
album artwork uses a fixed-size GPU widget so downloaded image dimensions do
not affect the grid, track rows, or player bar.

`scripts/gui_e2e.py` drives actual GTK controls with AT-SPI and keyboard input on
Wayland. It checks three window sizes, library navigation, real silent stream
decoding, player controls, search, empty results, palette reloads, sign-out state,
and clean exit. It uses a private session copy; the real saved account session
and desktop theme are preserved. Screenshots and reports are gitignored under
`../test-results/`; `../test-results/latest-gui/report.json` identifies the most
recent successful run. These implementation updates supersede unfinished UI
items in the original static investigation below.

## Implementation update — 2026-10-17

The initial workbench described below has been extended with an independent
terminal client (`src/online.rs`, `./Play.sh`) using `tidalrs` 0.6.2. Browser
device authorization succeeded with the user's account. Library calls for
favorite tracks, user playlists and saved albums succeeded. A real authenticated
HIGH-quality stream loaded and decoded through mpv, with its playback clock
advancing during a silent check. No user credentials or signed stream URLs are
included in this report. Sessions are stored outside the repository, in a
private state directory with atomic mode-0600 token files.

The terminal milestone resolves streams on demand, supports unencrypted BTS and DASH
manifests, and can continue through the current track page. It does not add
MPRIS, offline storage, DRM decoding, or verified lossless output.
Five offline tests and the original local mpv smoke check pass. The remaining
work listed in the original snapshot is historical and should be read with this
update. Third-party dependencies and protocol references are recorded in
`../THIRD_PARTY.md`.

Snapshot: 2026-10-17. Target: official Windows desktop installer downloaded from
https://download.tidal.com/desktop/TIDALSetup.exe, linked by
https://offer.tidal.com/download. No installer or proprietary executable was run.

## Identity

| Artifact | Observed identity |
| --- | --- |
| Installer | TIDALSetup.exe, 194 MB approximately, PE32 Squirrel bootstrapper with embedded ZIP |
| Package | TIDAL-2.43.2-full.nupkg; 237 ZIP entries |
| package.json | TIDAL Desktop 2.43.2; entrypoint app/main.js |
| app/buildConfig.json | appVersion 2.43.2.21, packageVersion 2.43.2, release variant |
| Installer SHA-256 | `550c12978f116187a616604bcc4da92e46119bf7f42eb56acef2d135a60cabf8` |
| app.asar SHA-256 | `64dd40af2feb1ce7f8b3f6e3dc6c4c69a0bcc962cb6aa9bd8b936fd3fdefdd87` |
| Windows TIDALPlayer.exe SHA-256 | `433ba5449f6a4e8cce35ad9f6ca10aa837120ba1c21ea2deaba8f6fe367c59b0` |

## Findings and evidence

These are static observations. They establish code shipped in this package,
not successful execution, service availability, or authenticated playback.

| Question | Finding | Status / source |
| --- | --- | --- |
| Application structure | Electron main process, sandboxed preload bridge, remote interface at `https://desktop.tidal.com/` | Answered for selected code: `windowsConfiguration.js`, `WindowController.js`; REA E1 |
| Interface bridge | Preload exposes `nativeInterface` and `NativePlayerComponent` through contextBridge | Shell text inspection of `app/clientInterface/index.js`; not fully analyzed with REA |
| Player transport | `Player.js` spawns the configured native process; writes commands plus OS newline to stdin; splits stdout at OS newline and removes a prefix through the first colon | Answered for wrapper: REA E1 |
| Player command schema | JSON objects with `command`; examples below | Shell text inspection of preload; snippets with byte offsets in `evidence/player-command-snippets.json` |
| Player signals | JSON objects with `signal`, parsed by PlayerSignalHandler; signal names include `version`, `media.state`, `media.currenttime`, `media.duration`, `media.volume`, `media.devices`, `media.error` | REA E1 plus selected source reading |
| Session boundary | Main process receives username, userId, countryCode, sessionId from the renderer; it does not implement a complete authentication flow in the inspected controller | Partial: `UserSessionController.js`; REA E1 |
| Linux playback | Windows native player cannot be assumed portable; mpv is a proposed independent backend | Inference; adapter smoke test is separate evidence below |
| DRM / subscription streaming | Unresolved; package has Widevine configuration and a Widevine error screen. No license exchange or subscription stream tested | Partial: config and window code; REA E1 |
| Audio device / exclusive / bit-perfect parity | Unresolved; bridge has device operations but backend semantics unverified | Partial: preload and signal enum |

E1 = retained REA application analysis
`ev_fe33b16c344d2fba63299730ad9e6a21fffce04e53b85166b2597b49838b9e30`.
Summary receipt:
`ev_bf151ff520bc876498c3d12b75ecb772c8905f1e5e7f48e69a3dad9020bc967b`.
Complete local evidence bundle: `evidence/rea-bundle.json` (about 365 MB;
excluded from version control). Analysis selected nine exact package paths;
9,679 inventoried files remained outside source analysis. Do not interpret
missing graph facts as absence. The earlier package-only inventory analysis is
also preserved in that bundle.

## Observed command examples

```json
{"command":"media.load","url":"https://example.com/audio.flac","streamFormat":"flac","encryptionKey":""}
{"command":"media.play"}
{"command":"media.pause"}
{"command":"media.stop"}
{"command":"media.seek","time":30}
{"command":"media.volume","volume":0.5}
```

The preload also constructs `media.preload`, `media.recover`,
`media.cancelpreload`, `media.device.list`, `media.device.select`,
`media.device.release`, and MQA decoder commands. They are not implemented in
the Rust subset. Example media URL and format are illustrative; format values,
time units, volume range and load/autoplay semantics were not verified against
the proprietary player.

## Rust implementation and its limits

The independently written Rust workbench implements a typed subset of those
commands, bounded newline framing, signal parsing, and a proposed mpv JSON IPC
adapter. It does not embed the proprietary code or assets. The adapter chooses
seek seconds, volume 0–1 mapped to mpv 0–100, and mpv's load behavior. These are
explicit design decisions, not established TIDAL native-player semantics.
`streamFormat` is retained in the protocol but mpv detects the media format.
Encrypted input is unsupported. Sending waits for mpv command acknowledgements;
the CLI does not turn mpv responses into TIDAL player signals. Command acceptance
alone does not establish successful decoding or audible output.

Tests: four Rust integration tests cover fragmented CRLF frames, preservation
of unknown signal fields, command validation, and malformed/oversized input.
`scripts/smoke_mpv.py` drives real mpv with a generated WAV and checks backend
properties after load/play/pause/seek/volume/stop (seek position tolerance 250 ms
for the buffered audio clock). It uses null output: this
verifies control and decoding, not audible output or bit-perfect delivery.
Clippy is unavailable in the installed Rust toolchain; no toolchain was changed.

## Next implementation stages

1. Native Rust UI (GTK4 is available locally), with queue and player state
   independent from authentication and the audio backend.
2. User-owned developer application credentials and the documented OAuth flow;
   token refresh and OS keyring storage. No credentials copied from the app.
3. Catalog, search, albums and playlists using verified API schemas.
4. Establish the supported authorized playback path on Linux before promising
   subscription streaming, maximum quality, offline playback, or DRM support.
5. Add native media keys/MPRIS, PipeWire device selection and measured playback
   tests. Device/exclusive semantics need their own investigation.

Public references: [TIDAL authorization](https://developer.tidal.com/documentation/api-sdk/api-sdk-authorization),
[official Web SDK](https://github.com/tidal-music/tidal-sdk-web),
[mpv JSON IPC](https://mpv.io/manual/stable/#json-ipc).
The SDK documents a separation between API and SDK capabilities; public API
access alone is not evidence that arbitrary native playback is supported.


## v0.2.0 discovery and album listening

The first working GTK build is preserved in local Git at `v0.1.0`. The next
version adds account-authenticated Explore pages, genre/mood/decade navigation,
separate album and track catalog search, and adding albums through the SDK's
favorite-album endpoint. Explore is read from the live v1 `pages/explore` feed;
its page links lead to actual editorial album and playlist lists. Unsupported
video/artist modules are omitted rather than given nonfunctional controls.

Album browsing loads all track pages before offering playback, preserving disc
order and using the existing lazy per-track stream resolution. A left-hand
artwork panel sits beside the tracks; its action says "Play this album". The
footer play control has a fixed square allocation and fully rounded border.
The native Cairo Vines visualization uses real `lavfi.astats.Overall.RMS_level`
metadata from an mpv audio filter, sampled every 80 ms. It is a level-reactive
visualization, not an FFT spectrum. Its animation freezes while paused, follows
theme colors and only repaints while visible. No desktop loopback is captured.

The native E2E suite now covers Explore-to-Jazz navigation, album search and
opening results, idempotent saving of an album already in the user's collection,
album playback, audio meter output, the circular button's geometry and narrow
album layout. Full Windows feature parity is still incomplete: artist pages,
editorial videos, offline downloads and other Windows-specific features remain
outside this increment.
