#!/usr/bin/env python3
"""Native local diagnostic review on private X11. Never submits feedback.

Retained panic text is a fixture; process panic/signal classification is covered
by the Rust crash_lifecycle integration test. A rejecting loopback proxy records
any unexpected network attempt without forwarding it.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import socketserver
import subprocess
import threading
import time

from history_fixture import write_completed_settings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    attempts = queue.Queue()

    class RejectProxy(socketserver.StreamRequestHandler):
        def handle(self):
            attempts.put(self.rfile.readline().decode().strip())
            self.wfile.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")

    with socketserver.ThreadingTCPServer(("127.0.0.1", 0), RejectProxy) as proxy:
        threading.Thread(target=proxy.serve_forever, daemon=True).start()
        proxy_url = f"http://127.0.0.1:{proxy.server_address[1]}"
        env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1",
               "HTTPS_PROXY": proxy_url, "https_proxy": proxy_url, "NO_PROXY": "", "no_proxy": "",
               "CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER": "1", "CAPTURES_NATIVE_LAYOUT_PROBE": "1"}
        for name in ("WAYLAND_DISPLAY", "ALL_PROXY", "all_proxy"):
            env.pop(name, None)
        children = []
        with (output / "processes.log").open("w") as log:
            def spawn(command, announce=False):
                child = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
                children.append(child)
                return child

            def run(*command):
                return subprocess.check_output(command, env=env, stderr=log, timeout=15)

            try:
                server = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x1000x24", "-nolisten", "tcp"], True)
                env["DISPLAY"] = ":" + server.stdout.readline().decode().strip()
                spawn(["openbox", "--sm-disable"])
                for appearance, has_panic in (("dark", True), ("light", True), ("dark", False), ("light", False)):
                    case = "panic" if has_panic else "unclean"
                    profile = output / f"{appearance}-{case}"
                    diagnostics = profile / ".crash-diagnostics"
                    diagnostics.mkdir(parents=True)
                    diagnostics.joinpath("current-session").write_text(f"captures-session-v1\n{int(time.time() * 1000)}")
                    if has_panic:
                        diagnostics.joinpath("last-panic").write_text("Fixture Rust panic at /home/diagnostic-person/fixture.rs")
                    settings = profile / "settings.json"
                    write_completed_settings(settings)
                    events = profile / "app.jsonl"
                    command = [str(binary), "--live", "--open-history", "--history-root", str(profile),
                               "--settings-file", str(settings), "--appearance", appearance]

                    def launch():
                        with events.open("w") as stream:
                            app = subprocess.Popen(command, env=env, stdout=stream, stderr=log)
                        children.append(app)
                        root = run("xdotool", "search", "--all", "--sync", "--onlyvisible", "--pid", str(app.pid),
                                   "--name", "^Capture History$").decode().splitlines()[0]
                        return app, root

                    app, root = launch()
                    window = run("xdotool", "search", "--all", "--sync", "--onlyvisible", "--pid", str(app.pid),
                                 "--name", "^Send Feedback$").decode().splitlines()[0]
                    run("xdotool", "windowmove", "--sync", window, "0", "0")

                    def controls():
                        probes = {}
                        for line in events.read_text().splitlines():
                            try:
                                event = json.loads(line)
                            except ValueError:
                                continue
                            if event.get("event") == "feedback-layout":
                                probes = event["detail"]["controls"]
                        return probes

                    def click(name):
                        deadline = time.monotonic() + 15
                        while time.monotonic() < deadline:
                            probes = controls()
                            rect = probes.get(name)
                            page = probes.get("Page")
                            if rect and page:
                                if page[1] <= rect[1] and rect[3] <= page[3]:
                                    run("xdotool", "windowactivate", "--sync", window, "mousemove", "--window", window,
                                        str((rect[0] + rect[2]) // 2), str((rect[1] + rect[3]) // 2), "sleep", ".1", "click", "1")
                                    time.sleep(.2)
                                    return
                                run("xdotool", "mousemove", "--window", window, str(page[2] - 16), str((page[1] + page[3]) // 2), "click",
                                    "4" if rect[1] < page[1] else "5")
                            time.sleep(.1)
                        raise AssertionError(f"control never became visible: {name}")

                    def clipboard_message():
                        click("Message")
                        run("xdotool", "key", "ctrl+a", "ctrl+c")
                        time.sleep(.2)
                        return run("xclip", "-selection", "clipboard", "-o").decode()

                    marker = diagnostics.joinpath("current-session").read_bytes()
                    forwarded = subprocess.run(command, env=env, stdout=subprocess.PIPE, stderr=log, timeout=15, check=True)
                    assert b'"event":"forwarded"' in forwarded.stdout
                    assert diagnostics.joinpath("current-session").read_bytes() == marker, "secondary changed owner marker"
                    click("Copy summary")
                    summary = run("xclip", "-selection", "clipboard", "-o").decode()
                    if has_panic:
                        assert "Previous session exception evidence" in summary
                        assert "~/fixture.rs" in summary and "diagnostic-person" not in summary
                    else:
                        assert "Previous session did not close normally" in summary
                        assert "no panic or OS exception was confirmed" in summary
                    assert attempts.empty(), "review/copy attempted network access"
                    run("import", "-window", window, str(output / f"crash-review-{case}-{appearance}.png"))
                    run("xdotool", "windowsize", "--sync", window, "460", "460")
                    time.sleep(.3)
                    click("Copy summary")
                    run("import", "-window", window, str(output / f"crash-review-minimum-{case}-{appearance}.png"))
                    run("xdotool", "windowsize", "--sync", window, "640", "700")
                    time.sleep(.3)
                    click("Message")
                    run("xdotool", "type", "My description")
                    click("Add to message")
                    assert clipboard_message() == "My description\n\n" + summary, "visible draft was not preserved"
                    run("xdotool", "windowactivate", "--sync", root, "key", "ctrl+q")
                    assert app.wait(timeout=20) == 0
                    assert not diagnostics.joinpath("current-session").exists(), "accepted quit left marker"
                    assert diagnostics.joinpath("unclean-session").exists(), "quit erased retained evidence"
                    app, root = launch()
                    window = run("xdotool", "search", "--all", "--sync", "--onlyvisible", "--pid", str(app.pid),
                                 "--name", "^Send Feedback$").decode().splitlines()[0]
                    click("Dismiss diagnostics")
                    assert not diagnostics.joinpath("unclean-session").exists()
                    assert not diagnostics.joinpath("previous-panic").exists()
                    assert diagnostics.joinpath("current-session").exists(), "dismiss erased current session"
                    app.terminate()
                    app.wait(timeout=20)
                    assert not diagnostics.joinpath("current-session").exists(), "SIGTERM left marker"
                    assert attempts.empty(), "local review/add/dismiss attempted a network request"
                print("PASS: dark/light review, redaction, secondary ownership, copy/add without network, retained evidence, dismiss and clean Quit/SIGTERM")
            finally:
                for child in reversed(children):
                    if child.poll() is None:
                        child.terminate()
                        try:
                            child.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            child.kill()
                            child.wait(timeout=5)
                proxy.shutdown()


if __name__ == "__main__":
    main()
