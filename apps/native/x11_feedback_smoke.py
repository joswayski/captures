#!/usr/bin/env python3
"""Native feedback input on private X11; a rejecting loopback proxy prevents delivery.

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
                    app = spawn([str(binary), "--live", "--history-root", str(output / "history"),
                        "--settings-file", str(output / f"{appearance}.json"), "--appearance", appearance])
                    window = run("xdotool", "search", "--sync", "--onlyvisible", "--pid", str(app.pid), "--name", "^Captures$").decode().splitlines()[0]
                    time.sleep(1)

                    def click(x, y):
                        run("xdotool", "windowactivate", "--sync", window,
                            "mousemove", "--window", window, str(x - 1), str(y),
                            "mousemove_relative", "1", "0", "sleep", ".1", "click", "1")
                        time.sleep(.2)

                    def screenshot(name):
                        run("import", "-window", window, str(output / f"feedback-{appearance}-{name}.png"))

                    click(196, 14)  # Preferences, then About/scroll-to-end.
                    click(90, 319)
                    click(847, 488)
                    screenshot("empty")
                    click(269, 633)  # Empty Send is disabled.
                    assert requests.empty(), "opening/empty Send performed a network request"
                    click(283, 222)  # Idea.
                    click(380, 355)
                    message = "The selector overlaps the toolbar on my second display."
                    run("xdotool", "type", "--clearmodifiers", "--delay", "5", message)
                    click(269, 633)
                    assert requests.get(timeout=5) == "CONNECT captur.es:443 HTTP/1.1"
                    screenshot("sending")
                    click(269, 633)  # Duplicate Send while pending.
                    assert requests.empty(), "pending request allowed a second send"
                    click(273, 62)  # Back preserves both draft and pending operation.
                    click(90, 319)
                    click(847, 488)
                    release.set()
                    time.sleep(.7)
                    screenshot("error")
                    click(380, 355)
                    run("xdotool", "key", "ctrl+a", "ctrl+c")
                    assert run("xclip", "-selection", "clipboard", "-o").decode() == message, "error/navigation lost the draft"
                    click(269, 633)  # Failure can retry immediately.
                    assert requests.get(timeout=5) == "CONNECT captur.es:443 HTTP/1.1"
                    time.sleep(.5)
                    run("xdotool", "key", "alt+F4")
                    assert app.wait(timeout=10) == 0
                    assert not list((output / "history").glob("*/metadata.json")), "feedback created capture media"
                    print(f"PASS {appearance}: no startup/empty send, explicit submission, busy gate, retained draft, offline retry, clean exit", flush=True)
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
