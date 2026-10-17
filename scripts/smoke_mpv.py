#!/usr/bin/env python3
"""Exercise the actual Rust binary against headless mpv and synthetic audio."""
import json
import math
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time
import wave

ROOT = Path(__file__).resolve().parents[1]


def main():
    with tempfile.TemporaryDirectory(prefix="tidal-rs-") as temp:
        temp = Path(temp)
        audio = temp / "tone.wav"
        with wave.open(str(audio), "wb") as wav:
            wav.setparams((1, 2, 8000, 0, "NONE", "not compressed"))
            wav.writeframes(b"".join(struct.pack("<h", int(1000 * math.sin(2 * math.pi * 440 * i / 8000))) for i in range(8000 * 20)))
        ipc = temp / "mpv.sock"
        player = subprocess.Popen([
            "mpv", "--no-config", "--idle=yes", "--ao=null", "--vo=null",
            "--pause=yes", f"--input-ipc-server={ipc}", "--no-terminal", f"--log-file={temp / 'mpv.log'}",
        ], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not ipc.exists():
                if player.poll() is not None or time.monotonic() > deadline:
                    player.terminate()
                    _, error = player.communicate(timeout=3)
                    log = (temp / "mpv.log").read_text(errors="replace")
                    raise RuntimeError("mpv did not create IPC socket: " + error.decode(errors="replace") + log[-4000:])
                time.sleep(0.02)

            def query(prop):
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(2)
                    client.connect(str(ipc))
                    client.sendall((json.dumps({"command": ["get_property", prop], "request_id": 42}) + "\n").encode())
                    with client.makefile("r") as lines:
                        for line in lines:
                            response = json.loads(line)
                            if response.get("request_id") == 42:
                                return response
                raise RuntimeError("mpv disconnected before response")

            def send(command, prop, expected):
                subprocess.run([str(ROOT / "target/debug/tidal-rs"), "send", str(ipc)],
                               input=json.dumps(command) + "\n", text=True, check=True)
                deadline = time.monotonic() + 3
                while True:
                    response = query(prop)
                    data = response.get("data")
                    matches = data == expected
                    if prop == "time-pos" and isinstance(data, (int, float)):
                        # mpv's audio clock can lag the requested seek position
                        # by the output buffer, including with ao=null.
                        matches = abs(data - expected) <= 0.25
                    if response.get("error") == "success" and matches:
                        break
                    if time.monotonic() > deadline:
                        raise AssertionError((command, prop, expected, response))
                    time.sleep(0.02)

            send({"command": "media.load", "url": audio.as_uri(), "streamFormat": "wav"}, "path", str(audio))
            send({"command": "media.volume", "volume": 0.25}, "volume", 25.0)
            send({"command": "media.play"}, "pause", False)
            send({"command": "media.pause"}, "pause", True)
            send({"command": "media.seek", "time": 3.0}, "time-pos", 3.0)
            send({"command": "media.stop"}, "idle-active", True)
            print("PASS: Rust → mpv load, volume, play, pause, seek, stop (synthetic WAV; null audio output)")
        finally:
            player.terminate()
            try:
                player.wait(timeout=3)
            except subprocess.TimeoutExpired:
                player.kill()
                player.wait()


if __name__ == "__main__":
    main()
