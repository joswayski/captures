import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

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


if __name__ == "__main__":
    unittest.main()
