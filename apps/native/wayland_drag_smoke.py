#!/usr/bin/env python3
"""Exercise the Wayland outbound file drag against an independent GTK3 client."""
import argparse
import json
import os
import select
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parent
MANIFEST = ROOT / "wayland_drag_probe" / "Cargo.toml"
RECEIVER = ROOT / "wayland_drag_receiver.py"


class Output:
    def __init__(self, process):
        self.process = process
        self.buffer = b""
        self.seen = []

    def wait(self, prefix, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            while b"\n" in self.buffer:
                raw, self.buffer = self.buffer.split(b"\n", 1)
                value = raw.decode(errors="replace").rstrip("\r")
                self.seen.append(value)
                if value.startswith(prefix):
                    return value
            ready, _, _ = select.select([self.process.stdout], [], [], deadline - time.monotonic())
            if not ready:
                break
            chunk = os.read(self.process.stdout.fileno(), 4096)
            if not chunk:
                break
            self.buffer += chunk
        status = self.process.poll()
        stderr = self.process.stderr.read().decode(errors="replace") if status is not None else ""
        raise RuntimeError(f"missing {prefix!r}; saw {self.seen}; status={status}; stderr={stderr}")


def stop(process):
    if process and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


def ipc(env, command):
    deadline = time.monotonic() + 5
    while True:
        result = subprocess.run(["swaymsg", "-q", command], env=env)
        if result.returncode == 0:
            return
        if time.monotonic() >= deadline:
            result.check_returncode()
        time.sleep(.05)


def drag(env, binary, source_output, expected_uri, outcome):
    options = ["--reject"] if outcome == "reject" else ["--no-finish"] if outcome in ("timeout", "disappear") else []
    receiver = subprocess.Popen(
        ["/usr/bin/python3", str(RECEIVER), *options],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    output = Output(receiver)
    try:
        output.wait("READY")
        ipc(env, '[title="drag-source"] move position 40 40')
        ipc(env, '[title="drag-receiver"] move position 440 40')
        # Wait for both configure transactions before injecting the pointer press.
        time.sleep(.5)
        injector = subprocess.Popen([binary, "inject", "reject" if outcome == "cancel" else "accept"], env=env)
        source_output.wait("STARTED")
        received = None if outcome == "cancel" else output.wait("RECEIVED")
        if received:
            assert json.loads(received.removeprefix("RECEIVED ")) == expected_uri, received
            assert output.wait("BYTES ") == "BYTES " + bytes(range(256)).hex()
        if outcome == "disappear":
            stop(receiver)
        finished = source_output.wait("FINISHED")
        injector.wait(timeout=5)
        return finished
    finally:
        stop(receiver)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target-dir", type=Path, default=None)
    args = parser.parse_args()
    for command in ("sway", "swaymsg", "cargo"):
        if not shutil.which(command):
            raise RuntimeError(f"missing {command}")
    target = args.target_dir or Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "wayland_drag_probe" / "target"))
    subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(MANIFEST), "--target-dir", str(target)], check=True)
    binary = str(target / "debug" / "captures-wayland-drag-probe")
    with tempfile.TemporaryDirectory(prefix="captures-wayland-drag-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        payload = root / "exact name-é.bin"
        payload.write_bytes(bytes(range(256)))
        expected_uri = "file://" + quote(str(payload), safe="/") + "\r\n"
        config = root / "sway.conf"
        config.write_text('output HEADLESS-1 resolution 900x500\nseat seat0 fallback true\nfocus_follows_mouse no\nfor_window [title="drag-(source|receiver)"] floating enable\n')
        env = os.environ.copy()
        for key in ("DISPLAY", "SWAYSOCK", "WAYLAND_DISPLAY"):
            env.pop(key, None)
        env.update(XDG_RUNTIME_DIR=str(runtime), XDG_SESSION_TYPE="wayland", GDK_BACKEND="wayland", WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman")
        log = (root / "sway.log").open("w")
        sway = subprocess.Popen(["sway", "--unsupported-gpu", "--config", str(config)], env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        source = None
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                sockets = list(runtime.glob("wayland-*")); ipc_sockets = list(runtime.glob("sway-ipc.*.sock"))
                if sockets and ipc_sockets: break
                if sway.poll() is not None: raise RuntimeError("sway exited")
                time.sleep(.05)
            else: raise RuntimeError("sway did not become ready")
            env.update(WAYLAND_DISPLAY=sockets[0].name, SWAYSOCK=str(ipc_sockets[0]))
            source = subprocess.Popen([binary, "source", str(payload)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            source_output = Output(source); source_output.wait("READY")
            accepted1 = drag(env, binary, source_output, expected_uri, "accept")
            rejected = drag(env, binary, source_output, expected_uri, "reject")
            cancelled = drag(env, binary, source_output, expected_uri, "cancel")
            timed_out = drag(env, binary, source_output, expected_uri, "timeout")
            disappeared = drag(env, binary, source_output, expected_uri, "disappear")
            subprocess.run([binary, "inject", "self"], env=env, check=True, timeout=5)
            source_output.wait("STARTED")
            self_drop = source_output.wait("FINISHED")
            assert self_drop == "FINISHED accepted=true own=true same_source=true", self_drop
            accepted2 = drag(env, binary, source_output, expected_uri, "accept")
            assert accepted1 == accepted2 == "FINISHED accepted=true own=false"
            assert rejected == cancelled == "FINISHED accepted=false own=false"
            assert timed_out == disappeared == "FINISHED accepted=false own=false"
            print(json.dumps({"independent_receiver": "GTK3", "same_source_process": True, "exact_payload_bytes": 256, "exact_uri": expected_uri, "accept_results": [accepted1, accepted2], "reject_result": rejected, "cancel_result": cancelled, "timeout_result": timed_out, "disappeared_result": disappeared, "self_drop": self_drop}, indent=2))
        finally:
            stop(source)
            if sway.poll() is None:
                os.killpg(sway.pid, signal.SIGTERM)
                try: sway.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(sway.pid, signal.SIGKILL); sway.wait(timeout=5)
            log.close()


if __name__ == "__main__":
    main()
