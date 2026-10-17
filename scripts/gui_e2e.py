#!/usr/bin/env python3
"""Real GTK/Wayland E2E checks through AT-SPI, with silent live streaming.

Run: python scripts/gui_e2e.py [--inspect]
Uses a private D-Bus session, accessibility bus, state copy and theme file.
The user's saved session and desktop theme are never modified.
"""
import ast
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def outer():
    original_state = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state")))
    session = original_state / "tidal-rs/session.json"
    if not session.exists():
        raise SystemExit("Sign in with ./Play.sh first; these tests use a copy of that session.")
    results=ROOT/"test-results"
    results.mkdir(exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix="gui-e2e-",dir=results))
    with tempfile.TemporaryDirectory(prefix="tidal-rs-e2e-private-", ignore_cleanup_errors=True) as tmp:
        private = Path(tmp)
        state = private / "state/tidal-rs"
        state.mkdir(parents=True, mode=0o700)
        shutil.copyfile(session, state / "session.json")
        (state / "session.json").chmod(0o600)
        runtime = private / "run"
        runtime.mkdir(mode=0o700)
        original = session.read_bytes()
        display = os.environ.get("WAYLAND_DISPLAY", "wayland-1")
        if not display.startswith("/"):
            display = str(Path(os.environ["XDG_RUNTIME_DIR"]) / display)
        env = {**os.environ, "XDG_STATE_HOME": str(private / "state"),
               "XDG_RUNTIME_DIR": str(runtime), "WAYLAND_DISPLAY": display,
               "TIDAL_E2E_OUTPUT": str(output), "GTK_A11Y": "atspi", "GIO_USE_VFS":"local",
               "GTK_USE_PORTAL":"0", "GDK_DEBUG":"no-portals",
               "TIDAL_E2E_DESKTOP_RUNTIME":os.environ["XDG_RUNTIME_DIR"]}
        sink=f"tidal_e2e_{os.getpid()}"
        module=subprocess.check_output(["pactl","load-module","module-null-sink",f"sink_name={sink}","sink_properties=device.description=TIDAL_E2E_Silent"],text=True).strip()
        env["TIDAL_TEST_SINK"]=sink
        env["PULSE_SERVER"]="unix:"+os.environ["XDG_RUNTIME_DIR"]+"/pulse/native"
        env.pop("AT_SPI_BUS_ADDRESS", None)
        try:
            with open(output / "harness.log", "w") as harness:
                result = subprocess.run(["dbus-run-session", "--", sys.executable, __file__, "--inner", *sys.argv[1:]], env=env, stdout=harness, stderr=harness)
        finally:
            subprocess.run(["pactl","unload-module",module],check=True)
        assert session.exists(),"The real session was removed"
        assert json.loads(session.read_bytes()).get("user_id")==json.loads(original).get("user_id"),"The real account changed"
        assert session.stat().st_mode & 0o777==0o600,"Real session permissions changed"
        (state / "session.json").unlink(missing_ok=True)
        if result.returncode==0 and "--inspect" not in sys.argv:
            latest=results/"latest-gui"
            temporary=results/f".latest-gui-{os.getpid()}"
            temporary.symlink_to(output.name,target_is_directory=True)
            temporary.replace(latest)
        print("E2E artifacts:", output)
        raise SystemExit(result.returncode)


def inner():
    address = subprocess.check_output([
        "gdbus", "call", "--session", "--dest", "org.a11y.Bus",
        "--object-path", "/org/a11y/bus", "--method", "org.a11y.Bus.GetAddress",
    ], text=True)
    os.environ["AT_SPI_BUS_ADDRESS"] = ast.literal_eval(address)[0]
    registry_log = open(Path(os.environ["TIDAL_E2E_OUTPUT"]) / "registry.log", "w")
    registry = subprocess.Popen(["/usr/lib/at-spi2-registryd", "--use-gnome-session"], stdout=registry_log, stderr=registry_log)
    for _ in range(50):
        owned = subprocess.check_output(["gdbus", "call", "--address", os.environ["AT_SPI_BUS_ADDRESS"], "--dest", "org.freedesktop.DBus", "--object-path", "/org/freedesktop/DBus", "--method", "org.freedesktop.DBus.NameHasOwner", "org.a11y.atspi.Registry"], text=True)
        if "true" in owned:
            break
        time.sleep(0.1)
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi, GLib
    Atspi.init()
    out = Path(os.environ["TIDAL_E2E_OUTPUT"])
    palette = Path.home() / ".local/state/omarchy/current/theme/colors.toml"
    theme_file = out / "theme.toml"
    theme_file.write_text(palette.read_text())
    app_env = {**os.environ, "TIDAL_APP_ID": "io.github.tidalrs.E2E", "TIDAL_GUI_SILENT": "1", "TIDAL_THEME_FILE": str(theme_file)}
    log = open(out / "app.log", "w")
    app = subprocess.Popen([str(ROOT / "target/debug/tidal-rs"), "gui"], cwd=ROOT, env=app_env, stdout=log, stderr=log)
    report = {"checks": [], "screenshots": []}

    def pump():
        context = GLib.MainContext.default()
        while context.pending():
            context.iteration(False)

    def wait(fn, message, timeout=30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            pump()
            if app.poll() is not None:
                raise AssertionError(f"Application exited ({app.returncode}): {message}")
            try:
                result = fn()
                if result:
                    return result
            except (GLib.Error, RuntimeError):
                pass
            time.sleep(0.12)
        raise AssertionError("Timed out: " + message)

    def find_app():
        desktop = Atspi.get_desktop(0)
        for i in range(desktop.get_child_count()):
            child = desktop.get_child_at_index(i)
            if child.get_process_id() == app.pid:
                return child

    def walk(node):
        yield node
        for i in range(node.get_child_count()):
            child = node.get_child_at_index(i)
            if child:
                yield from walk(child)

    def nodes():
        pump()
        return list(walk(application))

    def find(name):
        matches=[n for n in nodes() if n.get_name()==name]
        return next((n for n in matches if n.get_role_name() in ("button","toggle button","entry","slider")),matches[0] if matches else None)

    def loaded_view(title):
        heading=next((n for n in nodes() if n.get_role_name()=="heading" and n.get_name()==title),None)
        return heading and not heading.get_state_set().contains(Atspi.StateType.BUSY)

    def track_rows():
        return [n for n in nodes() if n.get_name().startswith("Play track: ") and n.get_role_name()=="button"]

    def click(name):
        node = wait(lambda: find(name), "control " + name)
        action = node.get_action_iface()
        if action and action.get_n_actions():
            assert action.do_action(0), f"Could not activate {name}"
        else:
            focus_window()
            assert node.get_component_iface().grab_focus(), f"Could not focus {name}"
            subprocess.run(["wtype", "-k", "Return"],check=True)

    def window(title=None):
        clients = json.loads(subprocess.check_output(["hyprctl", "clients", "-j"],env={**os.environ,"XDG_RUNTIME_DIR":os.environ["TIDAL_E2E_DESKTOP_RUNTIME"]}))
        return next((c for c in clients if c["pid"] == app.pid and (title is None or c["title"]==title)), None)

    def capture(name,title=None):
        c = window(title)
        x, y = c["at"]
        w, h = c["size"]
        file = out / (name + ".png")
        subprocess.run(["grim", "-g", f"{x},{y} {w}x{h}", str(file)], check=True)
        if str(file) not in report["screenshots"]:
            report["screenshots"].append(str(file))
        return file

    desktop_env={**os.environ,"XDG_RUNTIME_DIR":os.environ["TIDAL_E2E_DESKTOP_RUNTIME"]}

    def dispatch(expression):
        result=subprocess.run(["hyprctl","dispatch",expression],env=desktop_env,capture_output=True,text=True)
        assert result.returncode==0,result.stdout+result.stderr

    def focus_window():
        dispatch('hl.dsp.focus({window='+json.dumps('address:'+window()['address'])+'})')

    def resize(width,height):
        c=window()
        target=json.dumps('address:'+c['address'])
        if not c["floating"]:
            dispatch(f'hl.dsp.window.float({{action="enable",window={target}}})')
        dispatch(f'hl.dsp.window.resize({{x={width},y={height},relative=false,window={target}}})')
        monitors=json.loads(subprocess.check_output(["hyprctl","monitors","-j"],env=desktop_env))
        m=next(m for m in monitors if m["focused"])
        x=int(m["x"]+(m["width"]/m["scale"]-width)/2)
        y=int(m["y"]+max(40,(m["height"]/m["scale"]-height)/2))
        dispatch(f'hl.dsp.window.move({{x={x},y={y},relative=false,window={target}}})')
        wait(lambda:abs(window()["size"][0]-width)<3 and abs(window()["size"][1]-height)<3,f"window resized to {width}x{height}; initial state={c}")
        time.sleep(0.35)

    def cards():
        return [n for n in nodes() if n.get_name().startswith("Open ") and n.get_role_name()=="button"]

    def rect(node):
        b=node.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
        return b.x,b.y,b.width,b.height

    def layout(min_columns):
        current=cards()
        assert current,"No album cards"
        boxes=[rect(n) for n in current]
        top=min(b[1] for b in boxes)
        row=[b for b in boxes if abs(b[1]-top)<2]
        assert len(row)>=min_columns,f"Too few grid columns: {len(row)} (expected at least {min_columns})"
        width,height=window()["size"]
        assert all(x>=0 and x+w<=width for x,_,w,_ in row),"Album grid overflows horizontally"
        first_labels=[n for n in walk(current[0]) if n.get_role_name()=="label"]
        assert all(rect(n)[2]<=170 for n in first_labels),"Album text extends beyond cover width"
        for name in ["Albums","Favorite tracks","Playlists","Search TIDAL catalog","Play / pause · Space","Volume","Next page"]:
            n=find(name)
            if n:
                x,y,w,h=rect(n)
                assert x>=0 and y>=0 and x+w<=width and y+h<=height,f"Control clipped: {name} {(x,y,w,h)} vs {(width,height)}"
        report["checks"].append(f"{width}x{height} layout: {len(row)} columns, constrained album labels, controls in bounds")

    def scroll_bottom():
        bars=[n for n in nodes() if n.get_role_name()=="scroll bar" and rect(n)[3]>100 and rect(n)[2]<40]
        assert bars,"No vertical browser scrollbar"
        value=bars[-1].get_value_iface()
        assert value and value.set_current_value(value.get_maximum_value()),"Could not scroll to end"

    def mpv_query(prop):
        sockets=list(Path(tempfile.gettempdir()).glob(f"tidal-rs-online-{app.pid}-*/mpv.sock"))
        if not sockets:return None
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(3);connection.connect(str(sockets[0]))
            connection.sendall((json.dumps({"command":["get_property",prop],"request_id":42})+"\n").encode())
            with connection.makefile("r") as lines:
                for line in lines:
                    reply=json.loads(line)
                    if reply.get("request_id")==42:return reply.get("data")

    def choose_combo(name,index):
        focus_window()
        choices=["High quality","Lossless","Max / Hi-Res"] if name=="Streaming quality" else ["Spectrum","Radial spectrum","Spectral ribbons","Waterfall"]
        node=find(name) or next((n for n in nodes() if n.get_role_name()=="combo box" and n.get_name() in choices),None)
        assert node,"Dropdown not found: "+name
        subprocess.run(["wtype","-M","ctrl","-k","k","-m","ctrl"],check=True)
        for _ in range(60):
            subprocess.run(["wtype","-k","Tab"],check=True)
            time.sleep(0.04);pump()
            if any(n.get_state_set().contains(Atspi.StateType.FOCUSED) for n in walk(node)):break
        else:raise AssertionError("Could not reach dropdown with keyboard: "+name)
        subprocess.run(["wtype","-k","Return"],check=True)
        wait(lambda:any(n.get_role_name()=="list item" for n in nodes()),"dropdown popup")
        subprocess.run(["wtype","-k","Home"],check=True)
        for _ in range(index):subprocess.run(["wtype","-k","Down"],check=True)
        subprocess.run(["wtype","-k","Return"],check=True)

    def enter_search(text):
        entry=wait(lambda:find("Search TIDAL catalog"),"search field")
        assert entry.get_editable_text_iface().set_text_contents(text),"Could not enter search"
        focus_window()
        subprocess.run(["wtype","-M","ctrl","-k","k","-m","ctrl"],check=True)
        subprocess.run(["wtype","-k","Return"],check=True)

    def dump():
        rows = []
        for n in nodes():
            try:
                component = n.get_component_iface()
                box = component.get_extents(Atspi.CoordType.WINDOW) if component else None
                rows.append({"name": n.get_name(), "role": n.get_role_name(), "description": n.get_description(),
                             "actions": n.get_action_iface().get_n_actions() if n.get_action_iface() else 0,
                             "box": [box.x,box.y,box.width,box.height] if box else None})
            except GLib.Error:
                pass
        (out / "accessibility-tree.json").write_text(json.dumps(rows, indent=2))
        return rows

    try:
        application = wait(find_app, "application registered on the private accessibility bus")
        wait(lambda:loaded_view("Home") and cards(),"personal Home page",timeout=45)
        assert find("Recently played songs") and find("Recently played albums"),"Home history sections missing"
        assert any(n.get_name().startswith("From your favorites · ") for n in nodes()),"Favorite artist recommendations missing"
        capture("00-home")
        report["checks"].append("Home shows favorite-artist recommendations and listening-history sections")
        click("Albums")
        wait(lambda:loaded_view("Your albums") and cards(),"album collection")
        wait(lambda: any(n.get_name().startswith("Open ") for n in nodes()), "album cards loaded")
        time.sleep(0.4)
        capture("01-albums")
        rows = dump()
        report["checks"].append("GTK app and live album collection exposed through AT-SPI")
        if "--inspect" in sys.argv:
            print(json.dumps(rows[:60], indent=2))
            return
        # Further interaction checks are kept below so a failure always leaves
        # a screenshot, accessibility tree and application log for diagnosis.
        resize(1420,900)
        layout(5)
        capture("02-wide-albums")
        resize(900,650)
        layout(2)
        capture("03-narrow-albums")
        resize(1180,820)
        layout(4)
        first=cards()[0].get_name()
        assert not find("Sign in"),"Already signed in, but redundant sign-in button is visible"
        assert not find("Next track").get_state_set().contains(Atspi.StateType.SENSITIVE),"Idle next-track control should be disabled"
        report["checks"].append("connected account UI and idle control states")
        assert not find("RUST  /  LINUX"),"Platform subtitle remains"
        assert not find("Next page") and not find("Previous page"),"Paging buttons remain"
        current=cards()
        scroll_bottom()
        wait(lambda:len(cards())>len(current),"album infinite loading appends a batch")
        assert cards()[0].get_name()==first,"Infinite loading replaced existing albums"
        report["checks"].append("console styling, removed subtitle, no paging controls and incremental album loading")
        click(first)
        wait(lambda:loaded_view(first[5:]) and track_rows(),"opened album tracks")
        assert find("Play this album"), "Album action has the wrong label"
        play_button=find("Play / pause · Space")
        _,_,pw,ph=rect(play_button)
        assert abs(pw-ph)<=1, f"Play button is not circular: {pw}x{ph}"
        resize(900,650)
        for row in track_rows():
            x,y,w,h=rect(row)
            assert x>=0 and x+w<=window()["size"][0], "Album track list overflows beside cover"
        resize(1180,820)
        # This album was opened from the existing saved collection. Saving it
        # again is idempotent and does not add or remove anything from the account.
        heart=wait(lambda:find("Remove album from favorites"),"already-saved album heart acknowledged")
        assert heart.get_state_set().contains(Atspi.StateType.CHECKED) or heart.get_state_set().contains(Atspi.StateType.PRESSED),"Saved album heart is not filled"
        assert not find("Save album"),"Old save button remains"
        report["checks"].append("saved album recognized before interaction and favorite heart overlays artwork")
        click("Play this album")
        wait(lambda:isinstance(mpv_query("time-pos"),(int,float)) and mpv_query("time-pos")>0.2,"album playback",timeout=40)
        spectrum=wait(lambda:find("Audio spectrum analyzer"),"real spectrum widget")
        import re
        def spectrum_live():
            text=spectrum.get_description()
            report["spectrum_metrics"]=text
            match=re.search(r"(\d+) FFT frames; ([\d.]+) draw fps",text)
            return match and int(match[1])>180 and 55<=float(match[2])<=65
        wait(spectrum_live,"real PCM FFT frames and 60 fps rendering",timeout=25)
        report["spectrum_metrics"]=spectrum.get_description()
        assert not find("Next visualizer"),"Old visualization modes remain"
        for index,name in enumerate(["Radial spectrum","Spectral ribbons","Waterfall"],1):
            choose_combo("Visualization",index)
            wait(lambda:name in spectrum.get_description(),"FFT visualization "+name)
            capture("04-"+name.lower().replace(" ","-"))
        choose_combo("Visualization",0)
        report["checks"].append("four frequency-driven visualization modes render and respond through the native selector")
        click("Home")
        wait(lambda:loaded_view("Home") and find("Recently played songs"),"Home recent listening history")
        history_file=next((Path(os.environ["XDG_STATE_HOME"])/"tidal-rs").glob("history-*.json"))
        entries=json.loads(history_file.read_text())
        assert entries and entries[0]["track"]["album"]["title"]==first[5:],"Played album was not recorded"
        capture("04-home-history")
        click("Open playing album")
        wait(lambda:loaded_view(first[5:]) and track_rows(),"bottom artwork opens the playing album page")
        assert not any(n.get_name()=="Now playing" and n.get_role_name()=="frame" for n in nodes()),"Bottom artwork still opens expanded view"
        report["checks"].append("Home recent songs/albums and bottom artwork opens current album page")
        click("Expand player")
        wait(lambda:any(n.get_name()=="Now playing" and n.get_role_name()=="frame" for n in nodes()),"bottom album art opens expanded player")
        wait(lambda:len([n for n in nodes() if n.get_name()=="Audio spectrum analyzer"])>=2,"expanded player has spectrum")
        wait(lambda:window("Now playing"),"expanded player mapped above main window")
        capture("04-expanded-player","Now playing")
        click("Close expanded player")
        wait(lambda:not any(n.get_name()=="Now playing" and n.get_role_name()=="frame" for n in nodes()),"expanded player closes")
        report["checks"].append("real FFT data, 60 fps spectrum and separate expanded player")
        capture("04-album-tracks")
        report["checks"].append("circular play button, narrow album layout, acknowledged favorite heart, album playback and real audio spectrum")
        click("Play / pause · Space")
        wait(lambda:mpv_query("pause") is True,"pause album")
        def control(*args):
            return subprocess.check_output([str(ROOT / "target/debug/tidal-rs"), "control", *args], env=app_env, text=True)
        def remote_status():
            return json.loads(control("status"))
        wait(lambda:remote_status().get("can_play"),"D-Bus playback metadata")
        control("hide")
        wait(lambda:not remote_status().get("visible") and not window(),"hidden player remains running")
        assert app.poll() is None,"Hiding exited the app"
        control("volume","31")
        wait(lambda:mpv_query("volume")==31,"volume control while hidden")
        control("seek","12")
        wait(lambda:abs(mpv_query("time-pos")-12)<2,"seek control while hidden")
        control("toggle-playback")
        wait(lambda:mpv_query("pause") is False,"resume while hidden")
        wait(lambda:remote_status().get("paused") is False,"hidden playback metadata continues updating")
        # One persistent D-Bus subscription must deliver real FFT data while
        # the GTK window is hidden, without spawning a process for each frame.
        import selectors
        stream=subprocess.Popen([str(ROOT / "target/debug/tidal-rs"),"control","spectrum"],env=app_env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        selector=selectors.DefaultSelector()
        selector.register(stream.stdout,selectors.EVENT_READ)
        pending=b""
        def spectrum_packets(seconds):
            nonlocal pending
            packets=[];deadline=time.monotonic()+seconds
            while time.monotonic()<deadline:
                if not selector.select(0.1): continue
                chunk=os.read(stream.stdout.fileno(),65536)
                if not chunk: raise AssertionError("Spectrum subscriber exited early")
                pending+=chunk
                while b"\n" in pending:
                    line,pending=pending.split(b"\n",1)
                    packets.append(json.loads(line))
            return packets
        try:
            packets=spectrum_packets(1.5)
            live=[p for p in packets if p.get("live")]
            assert len(live)>=35,f"Too few real spectrum frames: {len(live)}"
            assert all(len(p["bins"])==64 and all(0<=b<=1 for b in p["bins"]) for p in live),"Invalid FFT frame"
            assert any(max(p["bins"])>0.02 for p in live),"Stream contains no audio energy"
            report["spectrum_stream_fps"]=round(len(live)/1.5,1)
            control("toggle-playback")
            wait(lambda:remote_status().get("paused") is True,"pause while hidden")
            stopped=spectrum_packets(0.7)
            assert any(p.get("live") is False and not any(p["bins"]) for p in stopped),"Paused spectrum did not clear"
        finally:
            stream.terminate();stream.wait(timeout=3);selector.close()
        report["checks"].append("persistent D-Bus spectrum subscription delivers real 64-band FFT while hidden and clears on pause")
        control("show")
        wait(lambda:window() and remote_status().get("visible"),"show restores the same player")
        control("toggle-window")
        wait(lambda:not remote_status().get("visible"),"toggle hides the player")
        control("toggle-window")
        wait(lambda:window() and remote_status().get("visible"),"toggle restores the player")
        report["checks"].append("D-Bus metadata, hide/show/toggle preserve process and playback; hidden pause/resume, seek and volume work")
        click("Explore")
        wait(lambda:loaded_view("Explore") and cards(),"live Explore categories")
        wait(lambda:(card:=find("Open Jazz")) and any(n.get_description()=="Artwork loaded" for n in walk(card)),"Jazz category artwork loaded",timeout=35)
        capture("04b-explore")
        click("Open Metal")
        wait(lambda:loaded_view("Metal") and cards(),"Metal discovery feed")
        click("New Albums")
        new_albums=wait(lambda:loaded_view("Metal · New Albums") and cards(),"new Metal albums")
        assert not track_rows(),"Album filter returned a track list"
        first_new=new_albums[0].get_name()
        scroll_bottom()
        wait(lambda:loaded_view("Metal · New Albums") and len(cards())>len(new_albums),"genre new releases load on scroll")
        assert cards()[0].get_name()==first_new,"Genre loading replaced the first batch"
        assert find("New Tracks"),"Genre new-track filter missing"
        capture("04c-metal")
        click("New Tracks")
        wait(lambda:loaded_view("Metal · New Tracks") and track_rows(),"new Metal tracks")
        capture("04d-metal-new-tracks")
        click(track_rows()[0].get_name())
        wait(lambda:mpv_query("pause") is False,"new genre track can play",timeout=40)
        click("Play / pause · Space")
        wait(lambda:mpv_query("pause") is True,"pause new genre track")
        report["checks"].append("loaded category artwork, genre new albums, new tracks and playable discovery queue")
        report["checks"].append("live Explore genre navigation and discovery cards")
        report["checks"].append("album pagination and card activation open the correct album")
        click("Favorite tracks")
        rows=wait(lambda:loaded_view("Favorite tracks") and track_rows(), "favorite track rows")
        time.sleep(0.2)
        capture("05-favorites")
        report["checks"].append("sidebar activation loads real favorite tracks")
        row=wait(lambda:track_rows(),"favorite track controls")[0]
        resize(900,650)
        assert all(rect(n)[3]<=44 for n in track_rows()),"Artwork inflated track-row height"
        capture("05b-narrow-favorites")
        resize(1180,820)
        player_bar_before=rect(find("Playback position"))
        click(row.get_name())
        wait(lambda:isinstance(mpv_query("time-pos"),(int,float)) and mpv_query("time-pos")>0.1,"live stream decoded and advanced",timeout=40)
        capture("06-online-playback")
        assert rect(find("Playback position"))==player_bar_before,"Artwork changed the player bar layout"
        click("Play / pause · Space")
        wait(lambda:mpv_query("pause") is True,"pause button controls mpv")
        position=find("Playback position").get_value_iface()
        assert position.set_current_value(15.0),"Could not set seek position"
        wait(lambda:abs((mpv_query("time-pos") or 0)-15)<0.5,"accessible seek reaches mpv")
        assert find("Volume").get_value_iface().set_current_value(22.0)
        wait(lambda:mpv_query("volume")==22.0,"volume slider reaches mpv")
        preferences=Path(os.environ["XDG_STATE_HOME"])/"tidal-rs/preferences.json"
        choose_combo("Streaming quality",1)
        wait(lambda:preferences.exists() and json.loads(preferences.read_text())["quality"]=="lossless","quality preference saved")
        wait(lambda:"flac" in str(mpv_query("audio-codec-name")).lower(),"lossless applies to the active track",timeout=45)
        wait(lambda:mpv_query("pause") is True,"quality switch preserves pause state")
        wait(lambda:abs((mpv_query("time-pos") or 0)-15)<1.0,"quality switch preserves position")
        assert mpv_query("volume")==22.0,"Quality switch lost volume"
        report["checks"].append("lossless preference persists and active playback changes to FLAC while preserving position/pause/volume")
        before=mpv_query("path")
        click("Next track")
        wait(lambda:mpv_query("path") not in (None,before),"next button changes streamed track",timeout=40)
        wait(lambda:isinstance(mpv_query("time-pos"),(int,float)) and mpv_query("time-pos")>0.1,"second track playback")
        click("Play / pause · Space")
        wait(lambda:mpv_query("pause") is True,"pause second stream")
        report["checks"].append("real stream playback, pause, accessible seek, volume, next-track and pause")
        click("Playlists")
        wait(lambda:loaded_view("Your playlists") and cards(),"playlist cards")
        playlist=cards()[0].get_name()
        click(playlist)
        wait(lambda:loaded_view(playlist[5:]) and track_rows(),"playlist tracks")
        capture("07-playlist")
        report["checks"].append("playlist card activation and track listing")
        entry=wait(lambda:find("Search TIDAL catalog"),"search input")
        assert entry.get_editable_text_iface().set_text_contents("Radiohe")
        focus_window()
        subprocess.run(["wtype","-M","ctrl","-k","k","-m","ctrl"],check=True)
        completions=wait(lambda:[n for n in nodes() if n.get_role_name()=="button" and n.get_name().startswith("Complete: ")],"catalog autocomplete",timeout=15)
        completion=completions[0].get_name()
        click(completion)
        wait(lambda:loaded_view("Results for “"+completion[len("Complete: "):]+"”") and track_rows(),"autocomplete selection submits a completed search")
        report["checks"].append("debounced autocomplete suggestions and choosing a suggestion")
        enter_search("Radiohead")
        wait(lambda:loaded_view("Results for “Radiohead”") and track_rows(),"catalog search results")
        click("Search albums")
        wait(lambda:loaded_view("Albums for “Radiohead”") and cards(),"album catalog search")
        capture("07b-album-search")
        album_result=cards()[0].get_name()
        click(album_result)
        wait(lambda:loaded_view(album_result[5:]) and track_rows(),"search result opens playable album")
        assert find("Save album to favorites") or find("Remove album from favorites"),"Search album has no favorite heart"
        report["checks"].append("catalog album search opens album with save action")
        enter_search("91287409871234678901234567890123456789")
        wait(lambda:find("Nothing here yet"),"empty search state")
        capture("08-empty-search")
        report["checks"].append("keyboard-submitted catalog search and empty results")
        click("Albums")
        wait(lambda:cards(),"album grid restored")
        theme_file.write_text('''background = "#f4f1ea"\ndark_background = "#e9e4da"\nlighter_background = "#e3ddcf"\nforeground = "#242730"\nmuted = "#657078"\naccent = "#356f82"\nselection = "#d7e5e7"\n''')
        from PIL import Image
        def light_theme_visible():
            path=capture("09-live-light-theme")
            image=Image.open(path).convert("RGB")
            return sum(count for count,color in image.getcolors(image.width*image.height) if max(abs(a-b) for a,b in zip(color,(244,241,234)))<=10)>image.width*image.height*0.15
        wait(light_theme_visible,"live theme reload",timeout=10)
        theme_file.write_text(palette.read_text())
        time.sleep(2.2)
        capture("10-restored-dark-theme")
        report["checks"].append("live light/dark palette reload without restarting or changing desktop theme")
        dispatch('hl.dsp.window.close({window='+json.dumps('address:'+window()['address'])+'})')
        assert app.wait(timeout=5)==0,"App failed to exit while saving settings"
        saved_preferences=json.loads(preferences.read_text())
        assert saved_preferences["volume"]==22.0 and saved_preferences["quality"]=="lossless","Settings lost on exit"
        app=subprocess.Popen([str(ROOT/"target/debug/tidal-rs"),"gui"],cwd=ROOT,env=app_env,stdout=log,stderr=log)
        application=wait(find_app,"restarted app")
        wait(lambda:loaded_view("Home") and cards(),"Home restored after restart",timeout=45)
        assert abs(find("Volume").get_value_iface().get_current_value()-22)<0.1,"Volume UI not restored"
        assert find("Lossless"),"Quality selector not restored"
        assert any(n.get_name()=="Open "+entries[0]["track"]["title"] for n in nodes()),"Recently played song missing after restart"
        click("Albums")
        wait(lambda:loaded_view("Your albums") and cards(),"restored album library")
        click(cards()[0].get_name())
        wait(lambda:find("Play this album"),"album ready after restart")
        click("Play this album")
        wait(lambda:mpv_query("audio-codec-name") in ("aac","flac"),"first stream decoded after restart",timeout=45)
        assert mpv_query("volume")==22.0,"Saved volume not used for new player"
        if mpv_query("audio-codec-name")=="aac":
            wait(lambda:find("HIGH ↓"),"lower delivered quality is acknowledged rather than labeled lossless")
        click("Favorite tracks")
        wait(lambda:loaded_view("Favorite tracks") and track_rows(),"known FLAC-capable favorites")
        click(track_rows()[0].get_name())
        wait(lambda:"flac" in str(mpv_query("audio-codec-name")).lower(),"saved quality respected for known FLAC-capable track after restart",timeout=45)
        report["checks"].append("volume, quality and local listening history survive a real app restart and are applied to playback")
        click("Sign out of TIDAL")
        wait(lambda:find("Sign in with TIDAL"),"signed-out view")
        assert not (Path(os.environ["XDG_STATE_HOME"])/"tidal-rs/session.json").exists(),"Test session survived sign-out"
        assert find("Ready when you are"),"Signed-out player metadata was not cleared"
        assert not find("Next page"),"Pagination leaked into signed-out state"
        capture("11-signed-out")
        report["checks"].append("sign-out clears only isolated test session and resets UI")
        assert find("Welcome back"),"Signed-out header still exposes old library information"
        assert not find("Search TIDAL catalog").get_state_set().contains(Atspi.StateType.SENSITIVE),"Signed-out search should be disabled"
        logs=(out/"app.log").read_text()
        assert "Gtk-WARNING" not in logs and "Gtk-CRITICAL" not in logs and "panicked" not in logs,logs
        report["checks"].append("no GTK layout/CSS warnings, criticals or Rust panics")
        # Close through the window manager's normal close request. The worker
        # must shut down without leaving a player behind.
        dispatch('hl.dsp.window.close({window='+json.dumps('address:'+window()['address'])+'})')
        assert app.wait(timeout=5)==0,"Application failed during normal close"
        report["checks"].append("normal window close exits cleanly")
        report["passed"]=True
    except Exception as error:
        report["passed"]=False
        report["failure"]=repr(error)
        raise
    finally:
        if app.poll() is None:
            try:
                capture("last-state")
                dump()
            except Exception:
                pass
            app.terminate()
            try:
                app.wait(timeout=5)
            except subprocess.TimeoutExpired:
                app.kill()
                app.wait()
        log.close()
        registry.terminate()
        registry.wait(timeout=3)
        registry_log.close()
        # SIGTERM bypasses Rust destructors. Reap only this test's mpv child.
        for directory in Path(tempfile.gettempdir()).glob(f"tidal-rs-online-{app.pid}-*"):
            ipc = directory / "mpv.sock"
            try:
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(1)
                    client.connect(str(ipc))
                    client.sendall(b'{"command":["quit"]}\n')
            except OSError:
                pass
            shutil.rmtree(directory, ignore_errors=True)
        (out / "report.json").write_text(json.dumps(report, indent=2))


if __name__ == "__main__":
    if "--inner" in sys.argv:
        inner()
    else:
        outer()
