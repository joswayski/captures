#!/usr/bin/env python3
"""Read PRIMARY from independent owners on private X11/headless Wayland.

Exercises the same-executable private helper, not physical desktop acceptance.
No installed profile is opened. Use x11_editor_smoke --text-input-only for UI.
"""
import argparse
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import time

from wayland_clipboard_smoke import wait_for_socket, stop_compositor


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
    process.wait(timeout=3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="captures-primary-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        base = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK"):
            base.pop(key, None)
        # The compositors and clipboard owners get their own XDG directories, so
        # the profile check below only sees what the helper itself created.
        tools = root / "tools"
        base.update(XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(tools / "config"),
                    XDG_DATA_HOME=str(tools / "data"), XDG_CACHE_HOME=str(tools / "cache"))
        private = dict(XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"),
                       XDG_CACHE_HOME=str(root / "cache"))
        owners = []
        servers = []
        logs = []
        try:
            log = (root / "xvfb.log").open("wb")
            logs.append(log)
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "800x600x24"],
                                    env=base, stdout=subprocess.PIPE, stderr=log)
            servers.append(xvfb)
            assert select.select([xvfb.stdout], [], [], 10)[0], "Xvfb readiness"
            x11 = dict(base, DISPLAY=":" + xvfb.stdout.readline().decode().strip())
            config = root / "sway.conf"
            config.write_text("output HEADLESS-1 resolution 800x600\nseat seat0 fallback true\n")
            log = (root / "sway.log").open("wb")
            logs.append(log)
            wayland = dict(base, XDG_SESSION_TYPE="wayland", WLR_BACKENDS="headless",
                           WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman")
            sway = subprocess.Popen(["sway", "--unsupported-gpu", "--config", str(config)],
                                    env=wayland, stdout=log, stderr=log)
            servers.append(sway)
            wayland["WAYLAND_DISPLAY"] = wait_for_socket(runtime, sway, root / "sway.log").name

            def helper(backend, env, parent=None):
                return subprocess.run([binary, "--native-primary-selection", backend,
                                       str(os.getpid() if parent is None else parent)],
                                      env=dict(env, **private), capture_output=True, timeout=4)

            def owner(backend, env, payload, primary):
                command = (["xclip", "-quiet", "-selection", "primary" if primary else "clipboard", "-i"]
                           if backend == "x11" else
                           ["wl-copy", "--foreground", "--type", "text/plain;charset=utf-8", *( ["--primary"] if primary else [])])
                process = subprocess.Popen(command, env=env, stdin=subprocess.PIPE,
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                owners.append(process)
                process.stdin.write(payload)
                process.stdin.close()
                # Confirm publication with the independent transport, not a sleep.
                read = (["xclip", "-selection", "primary" if primary else "clipboard", "-o"]
                        if backend == "x11" else ["wl-paste", "--no-newline", *( ["--primary"] if primary else [])])
                deadline = time.monotonic() + 3
                while True:
                    result = subprocess.run(read, env=env, capture_output=True, timeout=2)
                    if result.returncode == 0 and result.stdout == payload:
                        break
                    assert process.poll() is None and time.monotonic() < deadline, "owner failed to publish"
                    time.sleep(.03)
                return process

            for backend, env in (("x11", x11), ("wayland", wayland)):
                owner(backend, env, b"ordinary CLIPBOARD must not be pasted", False)
                for payload in ("PRIMARY α\nsecond row".encode(), "é".encode() * 32768, b"", b"x" * 65537):
                    process = owner(backend, env, payload, True)
                    for _ in range(2):
                        result = helper(backend, env)
                        if len(payload) <= 65536:
                            assert result.returncode == 0 and result.stdout == payload, (
                                backend, len(payload), result.returncode, result.stdout[:80], result.stderr)
                        else:
                            assert result.returncode != 0 and result.stdout == b"", "overflow must not truncate"
                    assert process.poll() is None, "read must not kill the selection owner"
                    stop(process)
                assert helper(backend, env, parent=1).returncode != 0, "wrong parent must fail closed"
                failed = dict(env)
                failed["DISPLAY" if backend == "x11" else "WAYLAND_DISPLAY"] = ":65530" if backend == "x11" else "missing-wayland"
                result = helper(backend, failed)
                assert result.returncode != 0 and not result.stdout, "backend failure must not paste CLIPBOARD"
                print(f"PASS {backend}: PRIMARY-only UTF-8, 64 KiB boundary, overflow, repeat reads, parent/backend rejection", flush=True)

            # A live but nonresponding Wayland peer holds initialization before
            # arboard reads. Inspect the helper's limits without a GUI/profile.
            with socket.socket(socket.AF_UNIX) as server:
                server.bind(str(runtime / "blocked-wayland"))
                server.listen()
                server.settimeout(3)
                process = subprocess.Popen([binary, "--native-primary-selection", "wayland", str(os.getpid())],
                                           env=dict(wayland, WAYLAND_DISPLAY="blocked-wayland", **private),
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                owners.append(process)
                connection, _ = server.accept()
                with connection:
                    limits = Path(f"/proc/{process.pid}/limits").read_text()
                    assert "Max address space         536870912" in limits, limits
                    assert "Max core file size        0" in limits, limits
                    assert "Max cpu time              2" in limits, limits
                    assert process.poll() is None
                stop(process)
                # The GUI/supervisor can die while arboard is still blocked.
                # Its child must be killed without relying on Rust Drop.
                parent = subprocess.Popen([sys.executable, "-c",
                    "import os,subprocess,sys,time; "
                    "child=subprocess.Popen([sys.argv[1],'--native-primary-selection','wayland',str(os.getpid())],"
                    "stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); "
                    "print(child.pid,flush=True); time.sleep(20)", str(binary)],
                    env=dict(wayland, WAYLAND_DISPLAY="blocked-wayland", **private), stdout=subprocess.PIPE)
                owners.append(parent)
                assert select.select([parent.stdout], [], [], 3)[0], "parent fixture readiness"
                child_pid = int(parent.stdout.readline())
                connection, _ = server.accept()
                try:
                    stop(parent)
                    deadline = time.monotonic() + 3
                    while True:
                        status = Path(f"/proc/{child_pid}/status")
                        if not status.exists() or "State:\tZ" in status.read_text():
                            break
                        assert time.monotonic() < deadline, "helper survived parent death"
                        time.sleep(.01)
                finally:
                    connection.close()
            created = [directory for directory in ("config", "data", "cache") if (root / directory).exists()]
            assert not created, f"private helper entered normal app startup (created {created})"
            print("PASS stalled helper resource ceilings, parent-death cleanup and no profile startup", flush=True)
        finally:
            for process in reversed(owners):
                stop(process)
            for process in reversed(servers):
                stop_compositor(process)
            for log in logs:
                log.close()


if __name__ == "__main__":
    main()
