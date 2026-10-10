#!/usr/bin/python3
"""Private-D-Bus regression harness for the Wayland GlobalShortcuts transport.

This is a protocol test, not compositor acceptance.  It deliberately removes
DISPLAY and uses disposable XDG directories and a private session bus.
"""

import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


DESKTOP = "org.freedesktop.portal.Desktop"
ROOT = "/org/freedesktop/portal/desktop"
GLOBAL = "org.freedesktop.portal.GlobalShortcuts"
REQUEST = "org.freedesktop.portal.Request"
SESSION = "org.freedesktop.portal.Session"
CONTROL = "org.captures.Test.GlobalShortcuts"
ACTIONS = ["new_capture", "region", "window", "display", "record_region",
           "record_window", "record_display"]
DEFAULT_IDS = ["display", "record_window", "new_capture"]
DEFAULT_TRIGGERS = ["Super+Print (desktop)", "Alt+9 (desktop)", "Desktop launch"]


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


def fixture(mode, log_path):
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    name = dbus.service.BusName(DESKTOP, bus)
    log_path = Path(log_path)
    loop = GLib.MainLoop()
    bus.call_on_disconnection(lambda _: loop.quit())
    if mode in ("host-owned-commands", "queued-command-revocation"):
        from x11_capture_smoke import ScreenSaver
        saver_name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus)
        saver = ScreenSaver(saver_name, "/org/freedesktop/ScreenSaver")

    def record(event, **fields):
        with log_path.open("a") as output:
            output.write(json.dumps({"event": event, **fields}) + "\n")

    def props(ids, triggers=None):
        triggers = triggers if triggers is not None else [f"Desktop {item}" for item in ids]
        return dbus.Array([dbus.Struct((item, dbus.Dictionary({"trigger_description": trigger}, signature="sv")),
                                      signature="sa{sv}") for item, trigger in zip(ids, triggers)], signature="(sa{sv})")

    class RequestObject(dbus.service.Object):
        @dbus.service.method(REQUEST, in_signature="", out_signature="")
        def Close(self):
            record("request-close", path=self._object_path)

        @dbus.service.signal(REQUEST, signature="ua{sv}")
        def Response(self, code, results):
            pass

    class SessionObject(dbus.service.Object):
        @dbus.service.method(SESSION, in_signature="", out_signature="")
        def Close(self):
            record("session-close", path=self._object_path)

        @dbus.service.signal(SESSION, signature="")
        def Closed(self):
            pass

    class Portal(dbus.service.Object):
        def __init__(self):
            super().__init__(name, ROOT)
            self.requests = []
            self.sessions = []
            self.session_path = None
            self.snapshot_ids = list(DEFAULT_IDS)
            self.snapshot_triggers = list(DEFAULT_TRIGGERS)
            self.screenshot_uri = None
            self.client = None

        def request(self, sender, options, phase, results, code=0):
            token = str(options["handle_token"])
            assert token.startswith("captures_") and token.removeprefix("captures_").isalnum()
            expected = f"{ROOT}/request/{sender[1:].replace('.', '_')}/{token}"
            returned = expected + "_wrong" if mode == f"wrong-{phase}-request" else expected
            request = RequestObject(name, expected)
            self.requests.append(request)
            record(phase, expected=expected, returned=returned)
            if mode == f"{phase}-method-error":
                raise dbus.exceptions.DBusException("fixture failure", name="org.freedesktop.portal.Error.Failed")
            if mode != f"pending-{phase}":
                # This is intentionally before the method reply.
                request.Response(code, results)
            return dbus.ObjectPath(returned)

        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
        def Get(self, interface, prop):
            assert interface == GLOBAL and prop == "version"
            return dbus.UInt32(1 if mode == "v1" else 2)

        @dbus.service.method(GLOBAL, in_signature="a{sv}", out_signature="o", sender_keyword="sender")
        def CreateSession(self, options, sender):
            self.client = sender
            session = f"{ROOT}/session/{sender[1:].replace('.', '_')}/{options['session_handle_token']}"
            self.session_path = session
            obj = SessionObject(name, session)
            self.sessions.append(obj)
            returned = session + "_wrong" if mode == "wrong-session" else session
            return self.request(sender, options, "create", {"session_handle": dbus.String(returned)},
                                1 if mode == "cancel" else 2 if mode == "deny" else 0)

        @dbus.service.method(GLOBAL, in_signature="oa(sa{sv})sa{sv}", out_signature="o", sender_keyword="sender")
        def BindShortcuts(self, session, shortcuts, parent, options, sender):
            assert str(session) == self.session_path and parent == ""
            received = [str(item[0]) for item in shortcuts]
            assert sorted(received) == sorted(ACTIONS), received
            if mode == "success":
                assert {str(item[0]): str(item[1]["preferred_trigger"]) for item in shortcuts} == {
                    "new_capture": "CTRL+ALT+F10", "region": "CTRL+SHIFT+F7", "window": "CTRL+SHIFT+F8",
                    "display": "CTRL+SHIFT+F9", "record_region": "CTRL+ALT+F11", "record_window": "ALT+F9",
                    "record_display": "CTRL+ALT+F12"}
            if mode == "empty":
                grant = props([])
            elif mode == "foreign":
                grant = props(["fixture_foreign"])
            elif mode == "duplicate":
                grant = props(["display", "display"], ["first", "second"])
            elif mode == "malformed":
                grant = props(["display"], [dbus.UInt32(7)])
            else:
                grant = props(self.snapshot_ids, self.snapshot_triggers)
            return self.request(sender, options, "bind", {"shortcuts": grant})

        @dbus.service.method(GLOBAL, in_signature="oa{sv}", out_signature="o", sender_keyword="sender")
        def ListShortcuts(self, session, options, sender):
            assert str(session) == self.session_path
            return self.request(sender, options, "list",
                                {"shortcuts": props(["foreign"]) if mode == "foreign-list" else
                                 props(self.snapshot_ids, self.snapshot_triggers)},
                                1 if mode == "cancel-list" else 0)

        @dbus.service.method(GLOBAL, in_signature="osa{sv}", out_signature="")
        def ConfigureShortcuts(self, session, parent, options):
            record("configure", session=str(session))
            if mode == "configure-error":
                raise dbus.exceptions.DBusException("configuration rejected", name="org.freedesktop.portal.Error.Failed")

        @dbus.service.signal(GLOBAL, signature="osta{sv}")
        def Activated(self, session, shortcut, timestamp, options):
            pass

        @dbus.service.signal(GLOBAL, signature="osta{sv}")
        def Deactivated(self, session, shortcut, timestamp, options):
            pass

        @dbus.service.signal(GLOBAL, signature="oa(sa{sv})")
        def ShortcutsChanged(self, session, shortcuts):
            pass

        @dbus.service.method(CONTROL, in_signature="s", out_signature="")
        def EmitActivated(self, shortcut):
            self.Activated(dbus.ObjectPath(self.session_path), shortcut, dbus.UInt64(11), {})

        @dbus.service.method(CONTROL, in_signature="s", out_signature="")
        def EmitDeactivated(self, shortcut):
            self.Deactivated(dbus.ObjectPath(self.session_path), shortcut, dbus.UInt64(12), {})

        @dbus.service.method(CONTROL, in_signature="s", out_signature="")
        def EmitWrongSession(self, shortcut):
            self.Activated(dbus.ObjectPath(self.session_path + "_foreign"), shortcut, dbus.UInt64(13), {})

        @dbus.service.method(CONTROL, in_signature="asas", out_signature="")
        def ReplaceSnapshot(self, ids, triggers):
            assert len(ids) == len(triggers)
            self.snapshot_ids, self.snapshot_triggers = map(str, ids), map(str, triggers)
            self.snapshot_ids, self.snapshot_triggers = list(self.snapshot_ids), list(self.snapshot_triggers)
            record("snapshot", ids=self.snapshot_ids, triggers=self.snapshot_triggers)

        @dbus.service.method(CONTROL, in_signature="asas", out_signature="")
        def EmitChanged(self, ids, triggers):
            self.ShortcutsChanged(dbus.ObjectPath(self.session_path), props(map(str, ids), map(str, triggers)))

        @dbus.service.method(CONTROL, in_signature="", out_signature="")
        def EmitClosed(self):
            self.sessions[-1].Closed()

        @dbus.service.method(CONTROL, in_signature="", out_signature="")
        def Stop(self):
            loop.quit()

        @dbus.service.method(CONTROL, in_signature="s", out_signature="")
        def SetScreenshot(self, uri):
            self.screenshot_uri = str(uri)

        @dbus.service.method("org.freedesktop.portal.Screenshot", in_signature="sa{sv}",
                             out_signature="o", sender_keyword="sender")
        def Screenshot(self, parent, options, sender):
            assert parent == "" and self.screenshot_uri
            return self.request(sender, options, "screenshot", {"uri": self.screenshot_uri})

        @dbus.service.method(CONTROL, in_signature="s", out_signature="")
        def EmitSpoof(self, member):
            # Directed signals bypass the bus's sender match rules.
            peer = dbus.bus.BusConnection(os.environ["DBUS_SESSION_BUS_ADDRESS"])
            if member == "Response":
                message = dbus.lowlevel.SignalMessage(self.requests[-1]._object_path, REQUEST, member)
                message.append(dbus.UInt32(0), dbus.Dictionary({"shortcuts": props(DEFAULT_IDS),
                                                               "session_handle": self.session_path},
                                                              signature="sv"), signature="ua{sv}")
            elif member == "Closed":
                message = dbus.lowlevel.SignalMessage(self.session_path, SESSION, member)
            elif member == "ShortcutsChanged":
                message = dbus.lowlevel.SignalMessage(ROOT, GLOBAL, member)
                message.append(dbus.ObjectPath(self.session_path), props([]), signature="oa(sa{sv})")
            else:
                assert member == "Activated"
                message = dbus.lowlevel.SignalMessage(ROOT, GLOBAL, member)
                message.append(dbus.ObjectPath(self.session_path), "display", dbus.UInt64(15),
                               dbus.Dictionary({}, signature="sv"), signature="osta{sv}")
            message.set_destination(self.client)
            peer.send_message(message)
            peer.flush()
            peer.close()

        @dbus.service.method(CONTROL, in_signature="", out_signature="")
        def BeginFlood(self):
            def emit():
                self.Activated(dbus.ObjectPath(self.session_path), "display", dbus.UInt64(20), {})
                self.Deactivated(dbus.ObjectPath(self.session_path), "display", dbus.UInt64(21), {})
                return True
            GLib.timeout_add(5, emit)

        @dbus.service.method(CONTROL, in_signature="s", out_signature="")
        def EmitWrongPath(self, shortcut):
            message = dbus.lowlevel.SignalMessage(ROOT + "/wrong", GLOBAL, "Activated")
            message.append(dbus.ObjectPath(self.session_path), shortcut, dbus.UInt64(14),
                           dbus.Dictionary({}, signature="sv"), signature="osta{sv}")
            bus.send_message(message)

        @dbus.service.method(CONTROL, in_signature="s", out_signature="")
        def EmitPeer(self, shortcut):
            peer = dbus.bus.BusConnection(os.environ["DBUS_SESSION_BUS_ADDRESS"])
            message = dbus.lowlevel.SignalMessage(ROOT, GLOBAL, "Activated")
            message.append(dbus.ObjectPath(self.session_path), shortcut, dbus.UInt64(15),
                           dbus.Dictionary({}, signature="sv"), signature="osta{sv}")
            peer.send_message(message)
            peer.flush()
            peer.close()

    portal = Portal()
    print("READY", flush=True)
    loop.run()


class Probe:
    def __init__(self, binary, env):
        self.process = subprocess.Popen([binary], env=env, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        self.buffer = b""

    def line(self, timeout=4):
        deadline = time.monotonic() + timeout
        while b"\n" not in self.buffer:
            if not select.select([self.process.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
                return None
            chunk = os.read(self.process.stdout.fileno(), 16384)
            assert chunk, self.process.stderr.read().decode()
            self.buffer += chunk
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def until(self, predicate, timeout=5):
        deadline = time.monotonic() + timeout
        seen = []
        while time.monotonic() < deadline:
            item = self.line(max(.01, deadline - time.monotonic()))
            assert item is not None, seen
            seen.append(item)
            if predicate(item):
                return item, seen
        raise AssertionError(seen)

    def command(self, command):
        self.process.stdin.write((command + "\n").encode())
        self.process.stdin.flush()
        _, seen = self.until(lambda item: item.get("event") == "quit" if command == "quit" else
                             item.get("event") == "command" and item.get("command") == command)
        return seen

    def no_actions(self, seconds=.15):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            item = self.line(deadline - time.monotonic())
            if item is None:
                return
            assert item.get("event") != "action", item

    def finish(self):
        if self.process.poll() is None:
            self.command("quit")
        stdout, stderr = self.process.communicate(timeout=4)
        assert self.process.returncode == 0, stderr.decode()
        return stdout, stderr


def read_log(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def launch_fixture(script, mode, root, env):
    log = root / f"{mode}-{time.monotonic_ns()}.jsonl"
    process = subprocess.Popen([sys.executable, script, "--fixture", mode, "--log", str(log)],
                               env=env, stdout=subprocess.PIPE)
    assert select.select([process.stdout], [], [], 5)[0]
    assert process.stdout.readline().strip() == b"READY"
    return process, log


def bound(probe):
    item, _ = probe.until(lambda x: x.get("event") == "status" and x["detail"]["state"] != "pending")
    return item["detail"]


def protocols(binary, root, env):
    script = str(Path(__file__).resolve())
    cases = []
    bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])

    def run(mode, check):
        service, log = launch_fixture(script, mode, root, env)
        probe = Probe(binary, env)
        try:
            check(probe, dbus.Interface(bus.get_object(DESKTOP, ROOT), CONTROL), log)
            probe.finish()
            events = read_log(log)
            assert any(x["event"] == "session-close" for x in events), events
            if mode in ("wrong-create-request", "wrong-bind-request", "create-method-error", "bind-method-error"):
                phase = "create" if "create" in mode else "bind"
                request = next(x for x in events if x["event"] == phase)
                assert {x["path"] for x in events if x["event"] == "request-close"} == {request["expected"]}, events
            cases.append(mode)
        finally:
            stop(probe.process)
            stop(service)

    def success(p, ctl, log):
        detail = bound(p)
        assert detail == {"state": "bound", "configurable": True,
                          "triggers": dict(zip(DEFAULT_IDS, DEFAULT_TRIGGERS)), "configuration_error": None}, detail
        ctl.EmitWrongSession("display"); ctl.EmitWrongPath("display"); ctl.EmitPeer("display")
        for member in ("Activated", "Closed", "ShortcutsChanged"):
            ctl.EmitSpoof(member)
        ctl.EmitDeactivated("display")
        p.no_actions()
        assert not any(x["event"] == "list" for x in read_log(log)), "peer changed membership"
        ctl.EmitActivated("window"); ctl.EmitDeactivated("window")
        p.no_actions()
        ctl.EmitActivated("display"); ctl.EmitActivated("display")
        p.no_actions()
        ctl.EmitDeactivated("display")
        action, seen = p.until(lambda x: x.get("event") == "action")
        assert action["action"] == "display" and sum(x.get("event") == "action" for x in seen) == 1
        ctl.EmitDeactivated("display"); ctl.EmitDeactivated("display")
        p.no_actions()
        for blocked, allowed in (("suspend", "resume"), ("disable", "enable")):
            ctl.EmitActivated("display"); p.no_actions()
            p.command(blocked)
            ctl.EmitDeactivated("display"); ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
            p.no_actions()
            p.command(allowed); ctl.EmitDeactivated("display")
            p.no_actions()
            ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
            assert p.until(lambda x: x.get("event") == "action")[0]["action"] == "display"
        ctl.EmitActivated("record_window")
        ctl.EmitActivated("display")
        p.no_actions()
        ctl.ReplaceSnapshot(["display", "new_capture"], ["Changed desktop trigger", "Desktop launch"])
        ctl.EmitChanged(["display"], ["partial ignored"])
        changed, seen = p.until(lambda x: x.get("event") == "status" and x["detail"].get("triggers") == {"display": "Changed desktop trigger", "new_capture": "Desktop launch"})
        assert changed["detail"]["configurable"] and not any(x.get("event") == "action" for x in seen)
        ctl.EmitDeactivated("display"); ctl.EmitDeactivated("record_window")
        ctl.EmitActivated("record_window"); ctl.EmitDeactivated("record_window")
        p.no_actions()
        ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
        assert p.until(lambda x: x.get("event") == "action")[0]["action"] == "display"
        ctl.EmitActivated("new_capture"); ctl.EmitDeactivated("new_capture")
        assert p.until(lambda x: x.get("event") == "action")[0]["action"] == "new_capture"
        assert any(x["event"] == "list" for x in read_log(log))

    run("success", success)

    def inspect(p, command="inspect"):
        return p.command(command)[-1]

    def queue(p, ctl, actions):
        before = inspect(p)["wakes"]
        for action in actions:
            ctl.EmitActivated(action); ctl.EmitDeactivated(action)
        # Observe the public wake callback, not a sleep that merely assumes the
        # worker received the commands while host dispatch is held.
        deadline = time.monotonic() + 4
        while True:
            state = inspect(p)
            assert state["capture_current"], state
            if state["wakes"] >= before + len(actions):
                return state
            assert time.monotonic() < deadline, state

    def take(p, action, generation, revision):
        seen = p.command("take")
        assert next(x for x in seen if x["event"] == "taken")["action"] == action, seen
        state = seen[-1]
        expected = {"generation": generation, "revision": revision}
        assert state["observed"] == state["applied"] == expected, state

    def host_owned(p, ctl, log):
        bound(p); p.command("hold")
        state = inspect(p, "selector")
        generation = state["observed"]["generation"]
        actions = ["display", "record_window", "display"]
        state = queue(p, ctl, actions)
        expected = {"generation": generation, "revision": 0}
        assert state["observed"] == state["applied"] == expected, (
            "portal worker advanced input ownership before host dispatch", state)
        for revision, action in enumerate(actions, 1):
            take(p, action, generation, revision)
        take(p, None, generation, 3)

        # Queued commands may not cross suppression, a capture-generation
        # change or a cancellation that happens after receipt but before take.
        for blocked, allowed in (("disable", "enable"), ("suspend", "resume")):
            queue(p, ctl, ["display"])
            p.command(blocked); p.command(allowed)
            take(p, None, generation, 3)
        queue(p, ctl, ["display"])
        state = inspect(p, "selector")
        assert state["observed"]["generation"] != generation, state
        generation = state["observed"]["generation"]
        take(p, None, generation, 0)
        queue(p, ctl, ["new_capture"])
        take(p, "new_capture", generation, 1)
        queue(p, ctl, ["display", "record_window"])
        p.command("cancel")
        take(p, None, generation, 1)
        take(p, None, generation, 1)

    run("host-owned-commands", host_owned)

    def queued_revocation(p, ctl, log):
        bound(p); p.command("hold")
        generation = inspect(p, "selector")["observed"]["generation"]
        queue(p, ctl, ["display", "record_window"])
        ctl.ReplaceSnapshot(["display"], ["Reconciled desktop trigger"])
        ctl.EmitChanged(["display"], ["partial ignored"])
        p.until(lambda x: x.get("event") == "status" and
                x["detail"].get("triggers") == {"display": "Reconciled desktop trigger"})
        take(p, None, generation, 0)
        queue(p, ctl, ["display"])
        take(p, "display", generation, 1)
        queue(p, ctl, ["display"])
        ctl.EmitClosed()
        assert bound(p)["state"] == "unavailable"
        take(p, None, generation, 1)
        p.command("retry")
        assert bound(p)["state"] == "bound"
        generation = inspect(p, "selector")["observed"]["generation"]
        take(p, None, generation, 0)
        queue(p, ctl, ["display"])
        take(p, "display", generation, 1)

    run("queued-command-revocation", queued_revocation)

    def empty(p, ctl, log):
        detail = bound(p)
        assert detail["state"] == "bound" and detail["triggers"] == {} and detail["configurable"]
        ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
        p.no_actions()
    run("empty", empty)

    for mode in ("cancel", "deny", "wrong-create-request", "wrong-bind-request", "wrong-session",
                 "foreign", "duplicate", "malformed", "create-method-error", "bind-method-error"):
        def unavailable(p, ctl, log, mode=mode):
            detail = bound(p)
            assert detail["state"] == "unavailable" and "Desktop global shortcuts" in detail["error"], (mode, detail)
        run(mode, unavailable)

    def v1(p, ctl, log):
        detail = bound(p); assert not detail["configurable"]
        events = p.command("configure")
        configured = next(x for x in events if x.get("event") == "configure")
        assert "does not support" in configured["error"] and not any(x["event"] == "configure" for x in read_log(log))
    run("v1", v1)

    def configure_error(p, ctl, log):
        before = bound(p)["triggers"]
        p.command("configure")
        status, _ = p.until(lambda x: x.get("event") == "status" and x["detail"].get("configuration_error"))
        assert status["detail"]["triggers"] == before
        ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
        assert p.until(lambda x: x.get("event") == "action")[0]["action"] == "display"
    run("configure-error", configure_error)

    for mode in ("pending-create", "pending-bind"):
        def pending_quit(p, ctl, log, mode=mode):
            assert p.line()["detail"]["state"] == "pending"
            deadline = time.monotonic() + 3
            while not log.exists() or not any(x["event"] == mode.removeprefix("pending-") for x in read_log(log)):
                assert time.monotonic() < deadline
                time.sleep(.02)
            ctl.EmitSpoof("Response")
            assert p.line(.15) is None, "peer completed or failed pending consent"
            ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
            p.no_actions()
            started = time.monotonic()
            p.command("quit")
            p.process.wait(timeout=4)
            assert time.monotonic() - started < 2
            events = read_log(log)
            assert any(x["event"] == "request-close" for x in events), events
            assert any(x["event"] == "session-close" for x in events), events
        run(mode, pending_quit)

    def close_retry(p, ctl, log):
        bound(p); ctl.EmitActivated("display"); p.no_actions(); ctl.EmitClosed()
        unavailable, _ = p.until(lambda x: x.get("event") == "status" and x["detail"]["state"] == "unavailable")
        assert "retry" in unavailable["detail"]["error"]
        ctl.EmitDeactivated("display"); ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
        p.no_actions()
        p.command("retry")
        detail, seen = p.until(lambda x: x.get("event") == "status" and x["detail"]["state"] == "bound")
        assert detail["detail"]["triggers"] == dict(zip(DEFAULT_IDS, DEFAULT_TRIGGERS))
        assert not any(x.get("event") == "action" for x in seen), seen
    run("trusted-close-and-retry", close_retry)

    for mode in ("list-method-error", "cancel-list", "foreign-list", "pending-list"):
        def refresh_failure(p, ctl, log, mode=mode):
            bound(p)
            ctl.EmitActivated("display")
            p.no_actions()
            ctl.EmitChanged(["display"], ["partial"])
            if mode == "pending-list":
                deadline = time.monotonic() + 3
                while not any(x["event"] == "list" for x in read_log(log)):
                    assert time.monotonic() < deadline
                    time.sleep(.02)
            else:
                assert bound(p)["state"] == "unavailable"
            ctl.EmitDeactivated("display"); ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
            p.no_actions()
            started = time.monotonic()
            p.command("quit")
            p.process.wait(timeout=4)
            assert time.monotonic() - started < 2
        run(mode, refresh_failure)

    # Owner loss must revoke routes and remain unavailable until explicit retry.
    service, log = launch_fixture(script, "success", root, env)
    p = Probe(binary, env)
    try:
        bound(p); stop(service)
        assert bound(p)["state"] == "unavailable"
        replacement, _ = launch_fixture(script, "success", root, env)
        try:
            time.sleep(.15)
            p.command("retry"); assert bound(p)["state"] == "bound"
        finally:
            stop(replacement)
        cases.append("owner-loss-and-retry")
    finally:
        stop(p.process); stop(service)

    # Repeated signals racing teardown must not hang or produce post-quit actions.
    def quit_race(p, ctl, log):
        bound(p)
        ctl.BeginFlood()
        started = time.monotonic()
        p.command("quit"); p.process.wait(timeout=4)
        assert time.monotonic() - started < 2
    run("quit-signal-race", quit_race)

    # The daemon must activate an installed public portal on first use.
    assert not bus.name_has_owner(DESKTOP)
    p = Probe(binary, env)
    try:
        assert bound(p)["state"] == "bound"
        p.finish()
        dbus.Interface(bus.get_object(DESKTOP, ROOT), CONTROL).Stop()
        cases.append("service-activation")
    finally:
        stop(p.process)
    print(json.dumps({"protocol_cases": cases, "private_bus": True, "display_unset": True,
                      "real_compositor_acceptance": False}))
    bus.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--fixture", metavar="MODE")
    parser.add_argument("--log", type=Path)
    args = parser.parse_args()
    if args.fixture:
        if not args.log:
            parser.error("--fixture requires --log")
        fixture(args.fixture, args.log)
        return
    if not args.binary:
        parser.error("--binary is required")
    with tempfile.TemporaryDirectory(prefix="captures-wayland-shortcuts-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"; runtime.mkdir(mode=0o700)
        env = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(key, None)
        env.update(XDG_RUNTIME_DIR=str(runtime), XDG_DATA_HOME=str(root / "data"),
                   XDG_CONFIG_HOME=str(root / "config"), XDG_CACHE_HOME=str(root / "cache"),
                   XDG_SESSION_TYPE="wayland", XDG_CURRENT_DESKTOP="fixture")
        services = root / "data/dbus-1/services"
        services.mkdir(parents=True)
        (services / f"{DESKTOP}.service").write_text(
            f"[D-BUS Service]\nName={DESKTOP}\nExec={sys.executable} {Path(__file__).resolve()} "
            f"--fixture success --log {root / 'activated.jsonl'}\n")
        daemon = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                                  env=env, stdout=subprocess.PIPE)
        try:
            env["DBUS_SESSION_BUS_ADDRESS"] = daemon.stdout.readline().decode().strip()
            env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
            protocols(str(args.binary.resolve()), root, env)
        finally:
            stop(daemon)


if __name__ == "__main__":
    main()
