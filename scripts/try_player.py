#!/usr/bin/env python3
"""Interactive local-media tester for the Rust playback adapter."""
import json
from pathlib import Path
import shlex
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    print("TIDAL Rust — playback prototype\n")
    print("This tests local audio through the Rust → mpv adapter.")
    print("TIDAL login, streaming, and a graphical interface are not implemented.\n")
    print("Commands:")
    print('  open /path/to/music.flac   Load a local audio file (quotes optional)')
    print("  play | pause | stop       Control playback")
    print("  seek 30                   Seek to 30 seconds")
    print("  volume 25                 Set volume to 25 percent")
    print("  quit                      Close the tester\n")
    with tempfile.TemporaryDirectory(prefix="tidal-rs-live-") as temp:
        ipc = Path(temp) / "mpv.sock"
        with open(Path(temp) / "mpv.log", "w") as log:
            player = subprocess.Popen([
                "mpv", "--no-config", "--idle=yes", "--no-video", "--no-terminal",
                "--volume=25", f"--input-ipc-server={ipc}",
            ], stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 5
                while not ipc.exists():
                    if player.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("Could not start mpv; check the audio backend")
                    time.sleep(0.02)
                print("Ready. Load a local audio file with open.\n", flush=True)
                while True:
                    try:
                        line = input("tidal-rs> ").strip()
                        verb, _, arg = line.partition(" ")
                        if verb in ("quit", "q", "exit"):
                            break
                        if not verb:
                            continue
                        if verb == "open":
                            if not arg:
                                raise ValueError("open requires a file path")
                            if arg.startswith(("'", '"')):
                                parts = shlex.split(arg)
                                if len(parts) != 1:
                                    raise ValueError("Enter one file path")
                                arg = parts[0]
                            path = Path(arg).expanduser().resolve(strict=True)
                            command = {"command": "media.load", "url": path.as_uri(), "streamFormat": path.suffix.lstrip(".")}
                        elif verb in ("play", "pause", "stop"):
                            command = {"command": "media." + verb}
                        elif verb == "seek":
                            command = {"command": "media.seek", "time": float(arg)}
                        elif verb == "volume":
                            command = {"command": "media.volume", "volume": float(arg) / 100}
                        else:
                            print("Use open, play, pause, stop, seek, volume, or quit.")
                            continue
                        result = subprocess.run([str(ROOT / "target/debug/tidal-rs"), "send", str(ipc)],
                                                input=json.dumps(command) + "\n", text=True, capture_output=True)
                        print("mpv accepted the command." if result.returncode == 0 else result.stderr.strip())
                    except (ValueError, OSError) as error:
                        print(error)
                    except (EOFError, KeyboardInterrupt):
                        break
            finally:
                player.terminate()
                try:
                    player.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    player.kill()
                    player.wait()


if __name__ == "__main__":
    main()
