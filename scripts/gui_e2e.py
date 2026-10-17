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
        env.pop("AT_SPI_BUS_ADDRESS", None)
        with open(output / "harness.log", "w") as harness:
            result = subprocess.run(["dbus-run-session", "--", sys.executable, __file__, "--inner", *sys.argv[1:]], env=env, stdout=harness, stderr=harness)
        assert session.read_bytes() == original, "The real session was modified"
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

    def window():
        clients = json.loads(subprocess.check_output(["hyprctl", "clients", "-j"],env={**os.environ,"XDG_RUNTIME_DIR":os.environ["TIDAL_E2E_DESKTOP_RUNTIME"]}))
        return next((c for c in clients if c["pid"] == app.pid), None)

    def capture(name):
        c = window()
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
                rows.append({"name": n.get_name(), "role": n.get_role_name(),
                             "actions": n.get_action_iface().get_n_actions() if n.get_action_iface() else 0,
                             "box": [box.x,box.y,box.width,box.height] if box else None})
            except GLib.Error:
                pass
        (out / "accessibility-tree.json").write_text(json.dumps(rows, indent=2))
        return rows

    try:
        application = wait(find_app, "application registered on the private accessibility bus")
        wait(lambda: find("Your albums"), "initial album view")
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
        click("Next page")
        wait(lambda:any(n.get_name().startswith("Page 2 of") for n in nodes()),"album page two")
        assert cards()[0].get_name()!=first,"Pagination repeated the first page"
        click("Previous page")
        wait(lambda:(items:=cards()) and items[0].get_name()==first,"back to page one")
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
        click("Save album")
        wait(lambda:find("Saved to your albums"),"idempotent album save acknowledged by live API")
        click("Play this album")
        wait(lambda:isinstance(mpv_query("time-pos"),(int,float)) and mpv_query("time-pos")>0.2,"album playback",timeout=40)
        meter=wait(lambda:mpv_query("af-metadata/meter"),"audio visualization metadata")
        assert "lavfi.astats.Overall.RMS_level" in meter,"Missing real RMS audio meter"
        assert find("Vines audio visualizer"),"Album visualizer missing"
        for viz in ["Neon Tunnel","Warp Drive","Plasma","Orbital","Laser Bloom","Vines"]:
            click("Next visualizer")
            wait(lambda:find(viz+" audio visualizer"),"visualizer mode "+viz)
            time.sleep(0.15)
            capture("04-viz-"+viz.lower().replace(" ","-"))
        click("Expand visualizer")
        wait(lambda:any(n.get_name()=="TIDAL Visualizer" and n.get_role_name()=="frame" for n in nodes()),"expanded visualizer window")
        click("Close visualizer")
        wait(lambda:not any(n.get_name()=="TIDAL Visualizer" and n.get_role_name()=="frame" for n in nodes()),"visualizer closes cleanly")
        report["checks"].append("six selectable audio-reactive visualizer modes and fullscreen window")
        capture("04-album-tracks")
        report["checks"].append("circular play button, narrow album layout, live idempotent save, album playback and real audio meter")
        click("Play / pause · Space")
        wait(lambda:mpv_query("pause") is True,"pause album")
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
        click("Next page")
        wait(lambda:loaded_view("Metal · New Albums") and (items:=cards()) and items[0].get_name()!=first_new,"genre new album pagination")
        click("Previous page")
        wait(lambda:loaded_view("Metal · New Albums") and (items:=cards()) and items[0].get_name()==first_new,"genre new albums return to page one")
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
        enter_search("Radiohead")
        wait(lambda:loaded_view("Results for “Radiohead”") and track_rows(),"catalog search results")
        click("Search albums")
        wait(lambda:loaded_view("Albums for “Radiohead”") and cards(),"album catalog search")
        capture("07b-album-search")
        album_result=cards()[0].get_name()
        click(album_result)
        wait(lambda:loaded_view(album_result[5:]) and track_rows(),"search result opens playable album")
        assert find("Save album"),"Search album cannot be saved"
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
