import importlib.util
import http.client
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from wayland_update_smoke import Host, Loopback, digest, fingerprint

spec = importlib.util.spec_from_file_location("smoke", Path(__file__).parent / "wgpu" / "smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class SmokeEvidenceTests(unittest.TestCase):
    def test_timeout_preserves_partial_output_and_still_fails(self):
        for stdout, stderr in [(b'{"event":"starting"}\n', b'GPU diagnostic\n'), (None, None)]:
            with self.subTest(stdout=stdout), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                timeout = subprocess.TimeoutExpired("native", 40, output=stdout, stderr=stderr)
                with patch.object(smoke.subprocess, "run", side_effect=timeout):
                    with self.assertRaises(subprocess.TimeoutExpired):
                        smoke.run(Path("native"), output, "idle", [])
                self.assertEqual((output / "idle.jsonl").read_bytes(), stdout or b"")
                self.assertEqual((output / "idle.stderr.txt").read_bytes(), stderr or b"")

    def test_wayland_updater_holds_partial_metadata_and_exact_package_boundaries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = b'{"schema":1,"notes":"held response"}'
            (root / "manifest.json").write_bytes(manifest)
            archive = root / "package"
            payload = bytes(range(251)) * 800
            archive.write_bytes(payload)
            fixture = Loopback(root, archive)
            try:
                for route, body, boundaries in (("/manifest.json", manifest, [1]),
                                                 ("/full", payload, [65536, 131072])):
                    fixture.plan(route=route, boundaries=boundaries)
                    connection = http.client.HTTPConnection("127.0.0.1", fixture.server.server_port, timeout=5)
                    connection.request("GET", route)
                    response = connection.getresponse()
                    self.assertEqual(int(response.getheader("Content-Length")), len(body))
                    received = b""
                    for index, boundary in enumerate(boundaries):
                        received += response.read(boundary - len(received))
                        deadline = time.monotonic() + 5
                        while len(fixture.held) <= index and time.monotonic() < deadline:
                            time.sleep(.01)
                        self.assertEqual(fixture.held[index], {"path": route, "bytes": boundary})
                        tail, finished = [], threading.Event()
                        if index == len(boundaries) - 1:
                            def read_tail():
                                tail.append(response.read())
                                finished.set()
                            reader = threading.Thread(target=read_tail)
                            reader.start()
                            self.assertFalse(finished.wait(.15), "held body was already complete")
                        fixture.holds[index][2].set()
                    self.assertTrue(finished.wait(5))
                    reader.join()
                    self.assertEqual(received + tail[0], body)
                    self.assertEqual(fixture.paths(), [route])
                    connection.close()
            finally:
                fixture.close()

    def test_wayland_updater_corrupts_served_bytes_without_mutating_operator_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "manifest.json").write_bytes(b'{"schema":1}')
            archive = root / "package"
            archive.write_bytes(b"signed full package bytes")
            before = fingerprint(root)
            fixture = Loopback(root, archive)
            try:
                for mode, route, path in (("signature-error", "/manifest.json", root / "manifest.json"),
                                          ("hash-error", "/full", archive)):
                    fixture.plan(mode)
                    connection = http.client.HTTPConnection("127.0.0.1", fixture.server.server_port, timeout=5)
                    connection.request("GET", route)
                    body = connection.getresponse().read()
                    self.assertEqual(len(body), path.stat().st_size)
                    served = root / "served"
                    served.write_bytes(body)
                    self.assertNotEqual(digest(served), digest(path))
                    served.unlink()
                    self.assertEqual(fingerprint(root), before)
                    connection.close()
            finally:
                fixture.close()

    def test_wayland_updater_does_not_accept_an_incomplete_state_report(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "host.jsonl"
            path.write_text('{"detail":{"title":"held"},"event":"update-notice"}\n'
                            '{"event":"update-notice","detail":null}')
            host = Host(0, path, None, {}, None)
            self.assertEqual(host.notice(), {"title": "held"})
            with path.open("a") as file:
                file.write("\n")
            self.assertIsNone(host.notice())


if __name__ == "__main__":
    unittest.main()
