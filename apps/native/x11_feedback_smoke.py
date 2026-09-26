#!/usr/bin/env python3
"""Native feedback window input on private X11; a rejecting loopback proxy prevents delivery.

The form reports control rectangles through CAPTURES_NATIVE_LAYOUT_PROBE, so
clicks follow the layout instead of fixed coordinates.

No production feedback is sent. Requires Xvfb, Openbox, xdotool, xclip and
ImageMagick. Shared HTTP success/cooldown tests use disposable loopback servers.
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
    requests = queue.Queue()
    release = threading.Event()

    class RejectProxy(socketserver.StreamRequestHandler):
        def handle(self):
            self.connection.settimeout(5)
            requests.put(self.rfile.readline().decode().strip())
            while self.rfile.readline().strip():
                pass
            release.wait(10)
            # Never establish a tunnel or forward a byte to the real service.
            self.wfile.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")

    with socketserver.ThreadingTCPServer(("127.0.0.1", 0), RejectProxy) as proxy:
        threading.Thread(target=proxy.serve_forever, daemon=True).start()
        proxy_url = f"http://127.0.0.1:{proxy.server_address[1]}"
        env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1",
               "HTTPS_PROXY": proxy_url, "https_proxy": proxy_url, "NO_PROXY": "", "no_proxy": ""}
        for variable in ("WAYLAND_DISPLAY", "ALL_PROXY", "all_proxy"):
            env.pop(variable, None)
        children = []
        with (output / "processes.log").open("w") as log:
            def spawn(command, announce=False):
                child = subprocess.Popen(command, env=env,
                    stdout=subprocess.PIPE if announce else log, stderr=log)
                children.append(child)
                return child

            def run(*command):
                return subprocess.check_output(command, env=env, stderr=log, timeout=10)

            try:
                server = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"], True)
                env["DISPLAY"] = ":" + server.stdout.readline().decode().strip()
                spawn(["openbox", "--sm-disable"])
                for appearance in ("dark", "light"):
                    release.clear()
                    write_completed_settings(output / f"{appearance}.json")
                    layout_path = output / f"{appearance}-app.jsonl"
                    with layout_path.open("w") as layout_out:
                        app = subprocess.Popen([str(binary), "--live", "--history-root", str(output / "history"),
                            "--settings-file", str(output / f"{appearance}.json"), "--appearance", appearance],
                            env={**env, "CAPTURES_NATIVE_LAYOUT_PROBE": "1"}, stdout=layout_out, stderr=log)
                    children.append(app)
                    root = run("xdotool", "search", "--sync", "--onlyvisible", "--pid", str(app.pid), "--name", "^Captures$").decode().splitlines()[0]
                    time.sleep(1)
                    layout = {"offset": 0, "controls": {}}

                    def controls():
                        # The form reports named rectangles (window points) as
                        # `feedback-layout` events whenever its layout changes.
                        with layout_path.open() as log_in:
                            log_in.seek(layout["offset"])
                            while (line := log_in.readline()).endswith("\n"):
                                layout["offset"] += len(line.encode())
                                try:
                                    event = json.loads(line)
                                except ValueError:
                                    continue
                                if event.get("event") == "feedback-layout":
                                    layout["controls"] = event["detail"]["controls"]
                        return layout["controls"]

                    def click(window, x, y):
                        run("xdotool", "windowactivate", "--sync", window,
                            "mousemove", "--window", window, str(x - 1), str(y),
                            "mousemove_relative", "1", "0", "sleep", ".1", "click", "1")
                        time.sleep(.2)

                    def open_feedback():
                        # Preferences > About > Send feedback "Open" (measured
                        # from the X11 sandbox root with its lifecycle-error panel).
                        click(root, 196, 14)
                        click(root, 90, 321)
                        time.sleep(.5)
                        click(root, 847, 501)
                        window = run("xdotool", "search", "--sync", "--onlyvisible", "--name", "^Send Feedback$").decode().splitlines()[-1]
                        # Shipping's 640×700 window; keep it fully on the 900 px screen.
                        run("xdotool", "windowmove", "--sync", window, "0", "0")
                        deadline = time.monotonic() + 10
                        while "Send" not in controls():
                            assert time.monotonic() < deadline, "feedback layout probe"
                            time.sleep(.1)
                        return window

                    def control(window, name):
                        # The footer scrolls with the form, like shipping: wheel
                        # the page until the named control is fully visible.
                        for _ in range(40):
                            rects = controls()
                            x0, y0, x1, y1 = rects[name]
                            page = rects["Page"]
                            if y0 >= page[1] and y1 <= page[3]:
                                return (x0 + x1) // 2, (y0 + y1) // 2
                            run("xdotool", "mousemove", "--window", window, str((page[0] + page[2]) // 2),
                                str((page[1] + page[3]) // 2), "click", "4" if y0 < page[1] else "5")
                            time.sleep(.2)
                        raise AssertionError(f"{name} never scrolled into view: {controls()}")

                    def press(window, name):
                        click(window, *control(window, name))

                    def screenshot(window, name):
                        run("import", "-window", window, str(output / f"feedback-{appearance}-{name}.png"))

                    window = open_feedback()
                    screenshot(window, "empty")
                    press(window, "Send")  # Empty Send is disabled.
                    assert requests.empty(), "opening/empty Send performed a network request"
                    press(window, "Category.Idea")
                    press(window, "Message")
                    message = "The selector overlaps the toolbar on my second display."
                    run("xdotool", "type", "--clearmodifiers", "--delay", "5", message)
                    press(window, "Send")
                    assert requests.get(timeout=5) == "CONNECT captur.es:443 HTTP/1.1"
                    screenshot(window, "sending")
                    press(window, "Send")  # Duplicate Send while pending.
                    assert requests.empty(), "pending request allowed a second send"
                    # Closing the window keeps both the draft and the pending send.
                    run("xdotool", "windowactivate", "--sync", window, "key", "alt+F4")
                    deadline = time.monotonic() + 10
                    while subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^Send Feedback$"],
                                         env=env, stdout=subprocess.DEVNULL, stderr=log).returncode == 0:
                        assert time.monotonic() < deadline, "feedback window did not close"
                        time.sleep(.1)
                    assert app.poll() is None, "closing feedback quit Captures"
                    window = open_feedback()
                    release.set()
                    time.sleep(.7)
                    assert "Status" in controls(), "offline failure has no status"
                    screenshot(window, "error")
                    press(window, "Message")
                    run("xdotool", "key", "ctrl+a", "ctrl+c")
                    assert run("xclip", "-selection", "clipboard", "-o").decode() == message, "error/close lost the draft"
                    press(window, "Send")  # Failure can retry immediately.
                    assert requests.get(timeout=5) == "CONNECT captur.es:443 HTTP/1.1"
                    time.sleep(.5)
                    screenshot(window, "retry")
                    run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
                    assert app.wait(timeout=10) == 0
                    assert not list((output / "history").glob("*/metadata.json")), "feedback created capture media"
                    print(f"PASS {appearance}: own window, no startup/empty send, explicit submission, busy gate, draft kept across close, offline retry, clean exit", flush=True)
                (output / "acceptance.json").write_text(json.dumps({
                    "passed": True, "appearances": ["dark", "light"],
                    "productionFeedbackSent": False,
                    "scope": "Private X11/software GL; rejecting loopback proxy; no hardware/AT acceptance."
                }, indent=2))
            finally:
                release.set()
                for child in reversed(children):
                    if child.poll() is None:
                        child.terminate()
                        try:
                            child.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            child.kill()
                            child.wait()
                proxy.shutdown()


if __name__ == "__main__":
    main()
