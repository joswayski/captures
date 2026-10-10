#!/usr/bin/python3
"""Query the real native crop provider over private AT-SPI D-Bus, not AccessKit.

Public Value writes must change geometry, not just return success. Action
enumeration and focus + injected keyboard input are separate evidence. Diagnostics
continue after contract failures. No save/export occurs; all profiles are disposable.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import subprocess
import time
import traceback

from history_fixture import write_completed_settings, write_history


HANDLES = ("Crop top left", "Crop top right", "Crop bottom right", "Crop bottom left",
           "Crop top", "Crop right", "Crop bottom", "Crop left")
FIELDS = ("Crop X", "Crop Y", "Crop width", "Crop height")
PREFIX = "org.a11y.atspi."


def expected_values(crop):
    x, y, width, height = crop
    return dict(zip(HANDLES, (x, x + width, x + width, x, y, x + width, y + height, x)))


def verify_handles(nodes, crop, limits, require_all=True):
    """Oracle uses fixture source geometry, not the provider's own expectations."""
    handles = [node for node in nodes if node["name"] in HANDLES]
    names = [node["name"] for node in handles]
    assert len(names) == len(set(names)), f"duplicate handles: {names}"
    if require_all:
        assert set(names) == set(HANDLES), f"expected eight handles, got {names}"
    else:
        assert handles, "no visible crop handles"
    for node in handles:
        name = node["name"]
        assert node["role"] == "slider", (name, node["role"])
        assert PREFIX + "Value" in node["interfaces"], (name, node["interfaces"])
        value = node["value"]
        assert value["CurrentValue"] == expected_values(crop)[name], (name, value, crop)
        assert (value["MinimumValue"], value["MaximumValue"]) == limits[name], (name, value, limits[name])
        assert value["MinimumIncrement"] == 1, (name, value)
        axis = "Y" if name in ("Crop top", "Crop bottom") else "X"
        description = node["description"]
        assert f"{axis} edge {expected_values(crop)[name]} source pixels" in description, (name, description)
        for label, dimension in zip(("X", "Y", "width", "height"), crop):
            assert re.search(rf"\b{label} {dimension}\b", description), (name, description, crop)
        assert node["bounds"][2] > 0 and node["bounds"][3] > 0, (name, node["bounds"])
    return handles


def verify_projected_bounds(handles, crop, source_size, image, origin):
    """Project fixture edges; origin is X11 SCREEN's client offset or WINDOW's zero."""
    x, y, w, h = crop
    width, height = source_size
    x0, y0, x1, y1, cx0, cy0, cx1, cy1 = image
    points = ((x, y), (x + w, y), (x + w, y + h), (x, y + h),
              (x + w / 2, y), (x + w, y + h / 2), (x + w / 2, y + h), (x, y + h / 2))
    expected = {}
    # Current native s-5 hit target is 12 points (shared/design.css).
    for name, (px, py) in zip(HANDLES, points):
        center_x, center_y = x0 + px / width * (x1 - x0), y0 + py / height * (y1 - y0)
        left, top = max(center_x - 6, cx0), max(center_y - 6, cy0)
        right, bottom = min(center_x + 6, cx1), min(center_y + 6, cy1)
        if right > left and bottom > top:
            expected[name] = (origin[0] + left, origin[1] + top, right - left, bottom - top)
    actual = {node["name"]: node["bounds"] for node in handles}
    assert actual.keys() == expected.keys(), f"visible handles {list(actual)}, expected {list(expected)}"
    for name, rect in actual.items():
        assert all(abs(a - b) <= 1 for a, b in zip(rect, expected[name])), (name, rect, expected[name])
    return expected


def fingerprints(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(root.rglob("*")) if path.is_file()}


class Provider:
    """Only public org.a11y.atspi interfaces; no app-side tree/probe values."""

    def __init__(self, bus, destination, window_coordinates=False):
        self.bus, self.destination = bus, destination
        self.window_coordinates = window_coordinates

    def interface(self, path, name):
        import dbus
        return dbus.Interface(self.bus.get_object(self.destination, path, introspect=False), PREFIX + name)

    def properties(self, path, name):
        import dbus
        obj = self.bus.get_object(self.destination, path, introspect=False)
        return dict(dbus.Interface(obj, "org.freedesktop.DBus.Properties").GetAll(PREFIX + name))

    def snapshot(self):
        import dbus
        deadline = time.monotonic() + 10
        while True:
            try:
                return self._snapshot()
            except dbus.DBusException as error:
                # Public trees update asynchronously; a child can disappear
                # or change interfaces between requests. Persistent errors fail.
                transient = ("org.freedesktop.DBus.Error.UnknownObject", "org.freedesktop.DBus.Error.UnknownInterface")
                if error.get_dbus_name() not in transient or time.monotonic() >= deadline:
                    raise
                time.sleep(.2)

    def _snapshot(self):
        import dbus
        import gi
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi
        result, pending, seen = [], ["/org/a11y/atspi/accessible/root"], set()
        while pending:
            path = str(pending.pop())
            if path in seen:
                continue
            seen.add(path)
            accessible = self.interface(path, "Accessible")
            props = self.properties(path, "Accessible")
            interfaces = [str(value) for value in accessible.GetInterfaces()]
            node = {"path": path, "name": str(props.get("Name", "")),
                    "description": str(props.get("Description", "")),
                    "role": Atspi.Role(int(accessible.GetRole())).value_nick, "interfaces": interfaces,
                    "states": [int(value) for value in accessible.GetState()], "actions": []}
            if node["role"] != "application":
                node["attributes"] = dict(accessible.GetAttributes())
            if PREFIX + "Value" in interfaces:
                node["value"] = self.properties(path, "Value")
            if PREFIX + "Text" in interfaces:
                node["text"] = str(self.interface(path, "Text").GetText(0, -1))
            if PREFIX + "Component" in interfaces:
                # SCREEN=0, WINDOW=1. Wayland SCREEN lacks a global origin:
                # retain it only as a diagnostic, never repair it with Sway IPC.
                component = self.interface(path, "Component")
                node["bounds"] = [int(v) for v in component.GetExtents(dbus.UInt32(int(self.window_coordinates)))]
                if self.window_coordinates:
                    node["screen_bounds_unsupported"] = [int(v) for v in component.GetExtents(dbus.UInt32(0))]
            if PREFIX + "Action" in interfaces:
                action = self.interface(path, "Action")
                entries = action.GetActions()
                node["action_details"] = [[str(value) for value in entry] for entry in entries]
                node["actions"] = [str(action.GetName(i)) for i in range(len(entries))]
            node["children"] = [[str(child[0]), str(child[1])] for child in accessible.GetChildren()]
            result.append(node)
            pending.extend(str(child[1]) for child in node["children"]
                           if str(child[0]) == self.destination)
        return result


def main():
    import dbus
    from dbus.mainloop.glib import DBusGMainLoop
    from gi.repository import GLib
    import threading

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--backend", choices=("x11", "wayland"), default="x11")
    parser.add_argument("--injector", type=Path, help="Wayland's existing captures-wayland-drag-probe")
    parser.add_argument("--appearance", choices=("dark", "light"), default="dark")
    parser.add_argument("--orca", action="store_true", help="Retain real Orca debug speech/event output; not human acceptance")
    parser.add_argument("--fault-disable-provider", action="store_true", help="Negative control: disable private ScreenReaderEnabled; must fail discovery")
    args = parser.parse_args()
    if args.orca and args.fault_disable_provider:
        parser.error("--orca would re-enable the provider; omit it for the negative control")
    wayland = args.backend == "wayland"
    if wayland and not args.injector:
        parser.error("--backend wayland requires --injector")
    injector = args.injector.resolve(strict=True) if wayland else None
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK", "AT_SPI_BUS_ADDRESS", "DBUS_SESSION_BUS_ADDRESS", "DBUS_SYSTEM_BUS_ADDRESS"):
        env.pop(key, None)
    for key in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"):
        directory = output / key.lower()
        directory.mkdir(mode=0o700)
        env[key] = str(directory)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11",
               LP_NUM_THREADS="2", CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1",
               CAPTURES_NATIVE_LAYOUT_PROBE="1", NO_AT_BRIDGE="0")
    if wayland:
        env.update(XDG_SESSION_TYPE="wayland", XDG_CURRENT_DESKTOP="sway", GDK_BACKEND="wayland",
                   WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1",
                   WLR_RENDERER="pixman")
    children, logs, cases = [], [], []
    result = {"passed": False, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "appearance": args.appearance, "backend": args.backend,
              "accepted_bound_coordinates": "WINDOW" if wayland else "SCREEN",
              "global_screen_bounds_accepted": not wayland,
              "transport": f"private session + accessibility D-Bus; real {args.backend} wgpu host",
              "cases": cases, "physical_acceptance": False}
    loop = None

    def spawn(name, command, announce=False, interactive=False):
        stdout = subprocess.PIPE if announce or interactive else (output / f"{name}.stdout.txt").open("w")
        stderr = (output / f"{name}.stderr.txt").open("w")
        logs.append(stderr)
        if not announce and not interactive:
            logs.append(stdout)
        child = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr,
                                 stdin=subprocess.PIPE if interactive else None, start_new_session=True)
        children.append(child)
        if announce:
            assert select.select([child.stdout], [], [], 60)[0], f"{name}: no readiness address"
            return child.stdout.readline().decode().strip()
        if interactive:
            assert select.select([child.stdout], [], [], 5)[0], f"{name}: no readiness"
            assert child.stdout.readline().strip() == b"READY", f"{name}: invalid readiness"
        return child

    def run(*command):
        return subprocess.check_output(command, env=env, timeout=40)

    def wait(predicate, message, timeout=30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(.15)
        raise AssertionError(message)

    def check(name, operation):
        try:
            detail = operation()
            cases.append({"name": name, "passed": True, "detail": detail})
            print(f"PASS {name}", flush=True)
        except AssertionError as error:
            cases.append({"name": name, "passed": False, "error": str(error)})
            print(f"FAIL {name}: {error}", flush=True)

    def windows():
        if wayland:
            from wayland_native_capture_smoke import windows as sway_windows
            return [str(node["id"]) for node in sway_windows(env)
                    if node.get("name", "").startswith("Captures Editor")]
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^Captures Editor"],
                               env=env, capture_output=True, text=True, timeout=5)
        assert found.returncode in (0, 1), found.stderr
        return found.stdout.split()

    def sway_editor():
        from wayland_native_capture_smoke import windows as sway_windows
        return next(node for node in sway_windows(env) if str(node["id"]) == editor)

    def arrange(width, height, initial=False):
        if wayland:
            run("swaymsg", f"[con_id={editor}] floating enable, resize set {width} {height}, move position 80 60, focus")
            wait(lambda: sway_editor()["rect"]["width"] == width
                 and sway_editor()["rect"]["height"] == height and sway_editor()["focused"], "editor resize/focus")
            time.sleep(1 if initial else .6)
        elif initial:
            run("xdotool", "windowmove", "--sync", editor, "80", "60", "windowsize", "--sync", editor, str(width), str(height),
                "windowactivate", "--sync", editor, "windowfocus", "--sync", editor, "sleep", "1")
        else:
            run("xdotool", "windowsize", "--sync", editor, str(width), str(height), "sleep", ".6")

    def inject(child, command, reply):
        child.stdin.write((command + "\n").encode())
        child.stdin.flush()
        assert select.select([child.stdout], [], [], 5)[0], "Wayland input stalled"
        assert child.stdout.readline().strip() == reply.encode(), "Wayland input acknowledgement"

    def key(name):
        if wayland:
            from wayland_keyboard_smoke import KEYS
            words = name.lower().split("+")
            code = KEYS["enter" if words[-1] == "return" else words[-1]]
            modifiers = "+".join(words[:-1]) or "none"
            inject(keyboard, f"{code} {modifiers}", f"KEYED {code} {modifiers}")
        else:
            run("xdotool", "key", name)

    def point(x, y, scroll=None):
        # Compositor coordinates are used for test input only, not AT-SPI bounds.
        rect = sway_editor()["rect"]
        suffix = f" {scroll}" if scroll is not None else ""
        inject(pointer, f'{round(rect["x"] + x)} {round(rect["y"] + y)} 1280 1200{suffix}',
               "SCROLLED" if scroll is not None else "CLICKED")

    def wheel(x, y, direction):
        if wayland:
            point(x, y, 78 if direction == "5" else -78)
            time.sleep(.35)
        else:
            run("xdotool", "mousemove", "--window", editor, str(x), str(y), "click", direction, "sleep", ".35")

    def save_snapshot(provider, name):
        nodes = provider.snapshot()
        (output / f"{name}.atspi.json").write_text(json.dumps(nodes, indent=2) + "\n")
        return nodes

    try:
        if not wayland:
            env["DISPLAY"] = ":" + spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0", "1280x1200x24", "-dpi", "96", "-nolisten", "tcp"], True)
            result["display"] = env["DISPLAY"]
        address = spawn("session-bus", ["dbus-daemon", "--session", "--nofork", "--print-address=1"], True)
        env["DBUS_SESSION_BUS_ADDRESS"] = env["DBUS_SYSTEM_BUS_ADDRESS"] = address
        DBusGMainLoop(set_as_default=True)
        session = dbus.bus.BusConnection(address)
        # Use the distro's actual accessibility launcher/status service and registry.
        launcher = session.get_object("org.a11y.Bus", "/org/a11y/bus")
        accessibility_address = str(dbus.Interface(launcher, "org.a11y.Bus").GetAddress())
        status = dbus.Interface(launcher, "org.freedesktop.DBus.Properties")
        status.Set("org.a11y.Status", "IsEnabled", dbus.Boolean(True))
        status.Set("org.a11y.Status", "ScreenReaderEnabled", dbus.Boolean(not args.fault_disable_provider))
        env["AT_SPI_BUS_ADDRESS"] = accessibility_address
        bus = dbus.bus.BusConnection(accessibility_address)
        registry = dbus.Interface(bus.get_object("org.a11y.atspi.Registry", "/org/a11y/atspi/registry"), PREFIX + "Registry")
        registry.RegisterEvent("object:property-change", dbus.Array([], signature="s"), "")
        registry.RegisterEvent("object:state-changed", dbus.Array([], signature="s"), "")
        events = (output / "atspi-events.jsonl").open("w")
        logs.append(events)

        def event(*values, **metadata):
            events.write(json.dumps({"time": time.monotonic(), **metadata, "args": values}, default=str) + "\n")
            events.flush()

        bus.add_signal_receiver(event, dbus_interface=PREFIX + "Event.Object", path_keyword="path",
                                member_keyword="member", sender_keyword="sender")
        loop = GLib.MainLoop()
        event_thread = threading.Thread(target=loop.run, daemon=True)
        event_thread.start()
        if wayland:
            version = run("sway", "--version").decode().strip()
            match = re.search(r"version (\d+)\.(\d+)", version)
            assert match and tuple(map(int, match.groups())) >= (1, 9), f"Sway 1.9+ required: {version}"
            config = output / "sway.conf"
            config.write_text('output HEADLESS-1 resolution 1280x1200\noutput * bg #234567 solid_color\n'
                              'seat seat0 fallback true\ndefault_border none\ndefault_floating_border none\n'
                              'for_window [title=".*"] floating enable\n')
            spawn("sway", ["sway", "--unsupported-gpu", "-c", str(config)])
            runtime = Path(env["XDG_RUNTIME_DIR"])
            socket = wait(lambda: next((p for p in runtime.glob("wayland-*") if p.is_socket()), None), "Wayland socket")
            env["WAYLAND_DISPLAY"] = socket.name
            env["SWAYSOCK"] = str(wait(lambda: next(iter(runtime.glob("sway-ipc.*.sock")), None), "Sway IPC"))
            run("dbus-update-activation-environment", "WAYLAND_DISPLAY", "SWAYSOCK")
            pointer = spawn("pointer", [str(injector), "pointer"], interactive=True)
            keyboard = spawn("keyboard", [str(injector), "keyboard"], interactive=True)
            result.update(sway=version, display_unset="DISPLAY" not in env,
                          injector_sha256=hashlib.sha256(injector.read_bytes()).hexdigest())
        else:
            spawn("openbox", ["openbox", "--sm-disable"])
        result["bus_status"] = {str(k): bool(v) for k, v in status.GetAll("org.a11y.Status").items()}
        # Static asymmetric PNG from the existing native fixture, then a known
        # GIF and small/large video. All are local FFmpeg-generated sources.
        write_history(output / "image-fixture")
        image = next((output / "image-fixture").glob("*/capture.png"))
        sources = output / "sources"
        sources.mkdir()
        gif = sources / "asymmetric.gif"
        run("ffmpeg", "-v", "error", "-loop", "1", "-i", str(image), "-t", "1", "-r", "5", str(gif))
        fixtures = [("video", 320, 180), ("gif", 640, 360), ("scroll-video", 1600, 900)]
        for label, width, height in fixtures:
            profile = output / label
            profile.mkdir()
            settings = profile / "settings.json"
            write_completed_settings(settings)
            settings_data = json.loads(settings.read_text())
            settings_data.update(appearance=args.appearance, show_mini_previews=False, auto_copy_to_clipboard=False)
            settings.write_text(json.dumps(settings_data))
            history = profile / "history"
            history.mkdir()
            source = gif if label == "gif" else sources / f"{label}.mp4"
            if label != "gif":
                run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"testsrc2=size={width}x{height}:rate=5:duration=1",
                    "-c:v", "mpeg4", "-q:v", "2", str(source))
            app = spawn(label, [str(binary), "--live", "--open-media", str(source), "--history-root", str(history),
                                "--settings-file", str(settings), "--quit-after", "900"])
            editor = wait(windows, "native editor window")[0]
            arrange(960, 1100, initial=True)

            def destination():
                daemon = dbus.Interface(bus.get_object("org.freedesktop.DBus", "/org/freedesktop/DBus"), "org.freedesktop.DBus")
                for name in bus.list_names():
                    if str(name).startswith(":") and daemon.GetConnectionUnixProcessID(name) == app.pid:
                        candidate = Provider(bus, str(name), wayland)
                        # GTK's file picker can register a second, empty root in
                        # the same host PID. Select the actual editor provider.
                        if candidate.interface("/org/a11y/atspi/accessible/root", "Accessible").GetChildren():
                            return str(name)
                return None

            provider = Provider(bus, wait(destination, "real host did not register on private AT-SPI bus", 10 if args.fault_disable_provider else 30), wayland)
            result.setdefault("providers", []).append({"fixture": label, "pid": app.pid, "destination": provider.destination})
            save_snapshot(provider, f"{label}-initial")
            def editor_tree():
                wait(lambda: any(n["name"] == "Crop recording" for n in provider.snapshot()),
                     "visible editor absent from public AT-SPI tree (History root is present)", 10)
            check(f"{label}/editor-public-tree", editor_tree)
            editor_available = cases[-1]["passed"]
            wait(lambda: list(history.glob("*/metadata.json")), "external source registered in disposable History")
            before_source, before_history = fingerprints(sources), fingerprints(history)

            def controls():
                latest = None
                for line in (output / f"{label}.stdout.txt").read_text().splitlines():
                    try:
                        item = json.loads(line)
                    except ValueError:
                        continue
                    if item.get("event") == "recording-editor-layout":
                        latest = item["detail"]["controls"]
                return latest or {}

            def visible(name):
                for _ in range(60):
                    rect = controls().get(name)
                    if rect:
                        x0, y0, x1, y1, cx0, cy0, cx1, cy1 = rect
                        left, top, right, bottom = max(x0, cx0), max(y0, cy0), min(x1, cx1), min(y1, cy1)
                        page = controls()["Page"]
                        fits = y1 - y0 <= page[7] - page[5]
                        whole = y0 >= cy0 and y1 <= cy1
                        if right - left > 2 and bottom - top > 2 and (whole or not fits):
                            return left, top, right, bottom
                        direction = "5" if (y1 > cy1 if fits else (y0 + y1) / 2 > cy1) else "4"
                        wheel(page[0] + 6, (page[1] + page[3]) // 2, direction)
                    else:
                        time.sleep(.15)
                raise AssertionError(f"control never visible: {name}")

            def press(name):
                since = None
                def idle():
                    nonlocal since
                    title = sway_editor()["name"] if wayland else run("xdotool", "getwindowname", editor).decode()
                    if "Working…" in title:
                        since = None
                    elif since is None:
                        since = time.monotonic()
                    return since is not None and time.monotonic() - since >= .5
                wait(idle, "native worker settled before input")
                previous, since = None, time.monotonic()
                def stable():
                    nonlocal previous, since
                    rect = visible(name)
                    if rect != previous:
                        previous, since = rect, time.monotonic()
                    return rect if time.monotonic() - since >= .5 else None
                # Automatic preview/comparison work can reflow the page after
                # the first idle title. Never click a stale or partial target.
                left, top, right, bottom = wait(stable, f"{name} visible and layout settled")
                if wayland:
                    run("swaymsg", f"[con_id={editor}] focus")
                    point((left + right) // 2, (top + bottom) // 2)
                    time.sleep(.3)
                else:
                    run("xdotool", "windowactivate", "--sync", editor, "windowfocus", "--sync", editor, "sleep", ".2",
                        "mousemove", "--window", editor, str((left + right) // 2), str((top + bottom) // 2),
                        "sleep", ".2", "mousedown", "1", "sleep", ".15", "mouseup", "1", "sleep", ".3")

            def copied_field(expected=None):
                if wayland:
                    run("wl-copy", "--clear")
                else:
                    subprocess.run(["xclip", "-selection", "clipboard", "-i"], env=env,
                                   input=b"waiting", check=True, timeout=5)
                def accepted():
                    if wayland:
                        key("ctrl+a")
                        time.sleep(.2)
                        key("ctrl+c")
                    else:
                        run("xdotool", "key", "ctrl+a", "ctrl+c")
                    command = ["wl-paste", "--no-newline"] if wayland else ["xclip", "-selection", "clipboard", "-o"]
                    read = subprocess.run(command, env=env, capture_output=True, timeout=5)
                    value = read.stdout.strip()
                    if read.returncode == 0 and value.isdigit() and (expected is None or value == str(expected).encode()):
                        return value
                    return None
                return int(wait(accepted, f"focused field accepted {expected}"))

            def fill(name, value):
                press(name)
                if wayland:
                    key("ctrl+a")
                    for digit in str(value):
                        key(digit)
                else:
                    run("xdotool", "key", "ctrl+a", "type", "--clearmodifiers", "--delay", "35", str(value))
                copied_field(value)
                key("Return")
                time.sleep(.3)

            def geometry():
                nodes = provider.snapshot()
                fields = {n["name"]: n["text"] for n in nodes if n["name"] in FIELDS and "text" in n}
                if len(fields) == 4:
                    return tuple(int(fields[name]) for name in FIELDS)
                assert not editor_available, "public numeric crop fields disappeared from a visible editor"
                # When the editor is missing, retain useful graphical diagnostics
                # without presenting clipboard reads as accessibility evidence.
                values = []
                for name in FIELDS:
                    press(name)
                    values.append(copied_field())
                    key("Return")
                    time.sleep(.3)
                return tuple(values)

            def stage(crop):
                for name, value in zip(FIELDS, crop):
                    fill(name, value)
                wait(lambda: geometry() == crop, f"numeric fields stage {crop}")

            press("Crop recording")
            press("Lock aspect ratio")  # Initial fixture entry uses free dimensions.
            crop = (width // 8, height // 9, width // 2, height * 4 // 9)
            stage(crop)
            press("Adjust crop")
            wait(lambda: "Crop size" in controls(), "actual graphical crop overlay")
            visible("Preview image")

            def free_limits(current):
                x, y, w, h = current
                return dict(zip(HANDLES, ((0, x + w - 2), (x + 2, width), (x + 2, width), (0, x + w - 2),
                                         (0, y + h - 2), (x + 2, width), (y + 2, height), (0, x + w - 2))))

            def render(name):
                time.sleep(.6)
                if wayland:
                    rect = sway_editor()["rect"]
                    run("grim", "-g", f'{rect["x"]},{rect["y"]} {rect["width"]}x{rect["height"]}', str(output / f"{name}.png"))
                else:
                    run("import", "-window", editor, str(output / f"{name}.png"))

            def inspect(name, current, require_all=True, limits=None):
                layout, since = {}, time.monotonic()
                def settled():
                    nonlocal layout, since
                    latest = controls()
                    if latest.get("Preview image") != layout.get("Preview image"):
                        layout, since = latest, time.monotonic()
                    return latest if time.monotonic() - since >= .6 else None
                # Match the provider to a settled frame, not the last pixels of
                # an animated page scroll. Never retry a failing bounds oracle.
                layout = wait(settled, "preview layout settled before public snapshot")
                nodes = save_snapshot(provider, name)
                (output / f"{name}.layout.json").write_text(json.dumps(layout, indent=2) + "\n")
                render(name)
                assert controls()["Preview image"] == layout["Preview image"], "preview reflowed during public snapshot"
                handles = verify_handles(nodes, current, limits or free_limits(current), require_all)
                # Independently project source edges to pixels using the existing
                # injector's image/clip rectangles, never its accessibility nodes.
                image = layout["Preview image"]
                clip = layout["Preview viewport"]
                if wayland:
                    # WINDOW projection never uses compositor/global offsets.
                    expected = verify_projected_bounds(handles, current, (width, height), image, (0, 0))
                    rect = sway_editor()["rect"]
                    return {"crop": current, "handles": len(handles), "clip": clip,
                            "expected_window_bounds": expected,
                            "screen_diagnostic": {"accepted": False, "sway_test_client_origin": [rect["x"], rect["y"]],
                                "reason": "Unix adapter has no compositor-independent Wayland global origin",
                                "handles": [{"name": node["name"], "window": node["bounds"],
                                             "screen": node["screen_bounds_unsupported"]} for node in handles]}}
                from Xlib import display
                connection = display.Display(env["DISPLAY"])
                try:
                    client = connection.create_resource_object("window", int(editor))
                    # xdotool geometry double-counts reparented Openbox frames;
                    # ask X11 to translate the actual client origin instead.
                    origin = connection.screen().root.translate_coords(client, 0, 0)
                finally:
                    connection.close()
                expected = verify_projected_bounds(handles, current, (width, height), image, (origin.x, origin.y))
                return {"crop": current, "handles": len(handles), "client_origin": [origin.x, origin.y],
                        "expected_screen_bounds": expected, "clip": clip}

            check(f"{label}/normal-values-bounds", lambda: inspect(f"{label}-normal", crop))
            def write_value(name, requested, expected):
                node = next((n for n in provider.snapshot() if n["name"] == name), None)
                assert node, f"{name} absent from public provider; Value write blocked"
                assert PREFIX + "Value" in node["interfaces"], (name, node["interfaces"])
                props = dbus.Interface(bus.get_object(provider.destination, node["path"], introspect=False),
                                       "org.freedesktop.DBus.Properties")
                observation = {"fixture": label, "name": name, "path": node["path"],
                    "request": requested, "before_value": node["value"]["CurrentValue"], "expected_crop": expected,
                    "method": "Properties.Set(org.a11y.atspi.Value, CurrentValue)", "actions": node["actions"]}
                result.setdefault("public_writes", []).append(observation)
                props.Set(PREFIX + "Value", "CurrentValue", dbus.Double(requested), signature="ssv")
                observation["setter_returned_success"] = True
                try:
                    wait(lambda: geometry() == expected, f"{name} Value={requested}: expected staged {expected}", 10)
                finally:
                    observation["actual_crop"] = geometry()
                nodes = provider.snapshot()
                changed_handles = [n for n in nodes if n["name"] in HANDLES]
                assert changed_handles, "public handles disappeared after Value write"
                for changed in changed_handles:
                    assert changed["value"]["CurrentValue"] == expected_values(expected)[changed["name"]], changed
                return observation

            def free_geometry(name, edge, base):
                x, y, w, h = base
                return ((edge, y, x + w - edge, h) if name in (HANDLES[0], HANDLES[3], HANDLES[7]) else
                        (x, edge, w, y + h - edge) if name == "Crop top" else
                        (x, y, w, edge - y) if name == "Crop bottom" else (x, y, edge - x, h))

            def public_nudges(names=HANDLES):
                # Independent one-source-pixel edge changes and their inverses.
                # Each pair restores the asymmetric fixture, including corners.
                observations = []
                for name in names:
                    edge = expected_values(crop)[name]
                    observations.append(write_value(name, edge + 1, free_geometry(name, edge + 1, crop)))
                    observations.append(write_value(name, edge, crop))
                return observations
            check(f"{label}/normal-public-value-nudges", public_nudges)
            stage(crop)  # Also restore after a failed partial pair.
            visible("Preview image")

            def public_limits():
                observations = []
                for name in HANDLES:
                    extent = height if name in ("Crop top", "Crop bottom") else width
                    for requested, bounded in zip((-1000, extent + 1000), free_limits(crop)[name]):
                        observations.append(write_value(name, requested, free_geometry(name, bounded, crop)))
                        observations.append(write_value(name, expected_values(crop)[name], crop))
                return observations
            check(f"{label}/public-value-saturation", public_limits)
            stage(crop)
            visible("Preview image")

            # Known 2:1 asymmetric fixture: the north-east and east maxima are
            # height-limited before the source's X boundary (not generic stops).
            press("Lock aspect ratio")
            visible("Preview image")
            x, y, w, h = crop
            locked_limits = dict(zip(HANDLES, ((0, x + w - 2), (x + 2, x + 2 * (y + h)),
                (x + 2, width), (0, x + w - 2), (0, y + h - 2),
                (x + 2, x + 4 * y + 2 * h), (y + 2, y + x + w / 2), (0, x + w - 2))))
            check(f"{label}/aspect-locked-reachable-bounds", lambda: inspect(f"{label}-locked", crop, limits=locked_limits))
            def locked_writes():
                observations = []
                # 2:1 fixture: horizontal edge movement couples height around
                # its center; NE hits the top before X reaches the source edge;
                # south hits X=0 before reaching the source bottom.
                for name, requested, expected in (
                    ("Crop right", x + w + 4, (x, y - 1, w + 4, h + 2)),
                    ("Crop top right", width + 1000, (x, 0, 2 * (y + h), y + h)),
                    ("Crop right", width + 1000, (x, 0, w + 4 * y, h + 2 * y)),
                    ("Crop bottom", height + 1000, (0, y, 2 * x + w, x + h))):
                    observations.append(write_value(name, requested, expected))
                    observations.append(write_value(name, expected_values(crop)[name], crop))
                return observations
            check(f"{label}/aspect-locked-public-values", locked_writes)
            stage(crop)
            press("Lock aspect ratio")
            visible("Preview image")

            if args.orca and label == "video":
                # Start against a settled editor, not the burst of setup typing.
                prefs = output / "orca-prefs"
                prefs.mkdir()
                # Keep audio/speech services owned and disposable too; do not
                # let Orca autospawn a daemon or connect to a desktop's sink.
                pulse_socket = output / "pulse.sock"
                env["PULSE_SERVER"] = "unix:" + str(pulse_socket)
                spawn("pulse", ["pulseaudio", "-n", "--daemonize=no", "--use-pid-file=no",
                    "--exit-idle-time=-1", "--log-target=stderr",
                    f"--load=module-native-protocol-unix socket={pulse_socket} auth-anonymous=1",
                    "--load=module-null-sink sink_name=crop_speech rate=48000 channels=2"])
                wait(pulse_socket.exists, "private speech audio socket")
                speech_config = output / "speech-config"
                speech_config.mkdir()
                (speech_config / "speechd.conf").write_text('DefaultLanguage "en"\nDefaultModule espeak-ng\n'
                    'AddModule "espeak-ng" "sd_espeak-ng" "/etc/speech-dispatcher/modules/espeak-ng.conf"\n'
                    'AudioOutputMethod "pulse"\nDisableAutoSpawn\n')
                speech_socket = output / "speech.sock"
                env["SPEECHD_ADDRESS"] = "unix_socket:" + str(speech_socket)
                (output / "speech-logs").mkdir()
                spawn("speech", ["speech-dispatcher", "--run-single", "--config-dir", str(speech_config),
                    "--socket-path", str(speech_socket), "--pid-file", str(output / "speech.pid"),
                    "--log-dir", str(output / "speech-logs"), "--timeout", "0"])
                wait(speech_socket.exists, "private speech dispatcher socket")
                orca = spawn("orca", ["orca", "--user-prefs", str(prefs), "--debug-file", str(output / "orca.debug.txt"),
                               "--enable", "speech", "--disable", "braille"])
                time.sleep(2)

            def focus_keyboard(name, key_name, expected):
                node = next((n for n in provider.snapshot() if n["name"] == name), None)
                assert node, f"{name} absent from public provider; GrabFocus + keyboard blocked"
                assert provider.interface(node["path"], "Component").GrabFocus(), f"GrabFocus {name} rejected"
                time.sleep(.4)
                key(key_name)
                time.sleep(.6)
                wait(lambda: geometry() == expected, f"{name} + {key_name}: expected {expected}, got {geometry()}")
                return {"input": f"public AT-SPI GrabFocus + injected {args.backend} key (NOT public adjustment)", "crop": geometry()}

            changed = (crop[0], crop[1], crop[2] + 1, crop[3])
            check(f"{label}/focus-keyboard-control", lambda: focus_keyboard("Crop right", "Right", changed))
            crop = geometry()
            visible("Preview image")
            arrange(760, 580)
            visible("Preview image")
            check(f"{label}/minimum-values-bounds", lambda: inspect(f"{label}-minimum", crop))
            check(f"{label}/minimum-public-value-nudges", public_nudges)
            stage(crop)
            visible("Preview image")
            changed = (crop[0], crop[1] + 1, crop[2], crop[3] - 1)
            check(f"{label}/minimum-focus-keyboard", lambda: focus_keyboard("Crop top", "Down", changed))
            crop = geometry()
            visible("Preview image")
            if label == "scroll-video":
                press("100%")
                viewport = visible("Preview viewport")
                if wayland:
                    point((viewport[0] + viewport[2]) // 2, (viewport[1] + viewport[3]) // 2, 234)
                    time.sleep(.6)
                else:
                    run("xdotool", "mousemove", "--window", editor, str((viewport[0] + viewport[2]) // 2), str((viewport[1] + viewport[3]) // 2),
                        "click", "--repeat", "3", "--delay", "150", "5", "sleep", ".6")
                check(f"{label}/scrolled-clipped-bounds", lambda: inspect(f"{label}-scrolled", crop, False))
                visible_handles = [n for n in provider.snapshot() if n["name"] in HANDLES]
                node = next((n for n in visible_handles if n["name"] in ("Crop left", "Crop top left", "Crop bottom left")), None)
                if node:
                    check(f"{label}/scrolled-public-value-nudges", lambda: public_nudges((node["name"],)))
                    changed = (crop[0] + 1, crop[1], crop[2] - 1, crop[3])
                    check(f"{label}/scrolled-focus-keyboard", lambda: focus_keyboard(node["name"], "Right", changed))
                else:
                    cases.append({"name": f"{label}/scrolled-focus-keyboard", "passed": False, "error": "no visible west handle"})

            def immutable():
                assert fingerprints(sources) == before_source, "source media changed without export"
                assert fingerprints(history) == before_history, "History changed without export"
                assert not (profile / "exports").exists() or not list((profile / "exports").iterdir()), "export published without request"
                return {"source_sha256": before_source, "history_sha256": before_history, "export_requested": False}

            check(f"{label}/immutable-source-history", immutable)
            if args.orca and label == "video":
                orca.terminate()
                orca.wait(timeout=10)
            # Quit only this disposable process; no saving or dirty-close consent.
            app.terminate()
            app.wait(timeout=20)
            wait(lambda: not windows(), "fixture editor teardown")
        result["passed"] = all(case["passed"] for case in cases)
    except Exception as error:
        result["fatal"] = f"{type(error).__name__}: {error}"
        print(result["fatal"], flush=True)
        (output / "failure.txt").write_text(traceback.format_exc())
        if "DISPLAY" in env:
            subprocess.run(["import", "-window", "root", str(output / "failure-desktop.png")], env=env, timeout=10)
        elif "WAYLAND_DISPLAY" in env:
            subprocess.run(["grim", str(output / "failure-desktop.png")], env=env, timeout=10)
    finally:
        if loop:
            loop.quit()
            event_thread.join(timeout=5)
        for child in reversed(children):
            # Each child and activated service has our own process group. Bus
            # activation descendants die too; never kill a desktop's real bus.
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        for child in reversed(children):
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
        for log in logs:
            log.close()
        if args.orca and (output / "orca.debug.txt").exists():
            # Orca buffers this file. Parse only after process exit flushes it.
            debug = (output / "orca.debug.txt").read_text()
            speech = [line for line in debug.splitlines() if "SPEECH OUTPUT" in line
                      or "SPEECH DISPATCHER: Speaking" in line or "Crop " in line and "object:" in line]
            (output / "orca-speech-events.txt").write_text("\n".join(speech) + "\n")
            result["orca"] = {"speech_event_lines": len(speech), "physical_listener": False,
                "registry_started": "ORCA: Starting registry" in debug,
                "speech_dispatcher_used": "SPEECH: Using speech server factory: speechdispatcherfactory" in debug,
                "editor_speech_confirmed": any("SPEECH OUTPUT" in line and "Crop " in line for line in speech),
                "handle_speech_confirmed": any("SPEECH OUTPUT" in line and any(name in line for name in HANDLES) for line in speech)}
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
