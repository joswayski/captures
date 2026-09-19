import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from history_fixture import history_entries

spec = importlib.util.spec_from_file_location("smoke", Path(__file__).parent / "wgpu" / "smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class SmokeEvidenceTests(unittest.TestCase):
    def test_history_observes_atomic_commit_not_temporary_or_backup_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            committed = root / "67e55044-10b1-426f-9247-bb680e5fe0c8"
            staging = root / f".{committed.name}.write.tmp"
            backup = root / f".{committed.name}.previous.bak"
            for path in [staging, backup]:
                path.mkdir()
                (path / "metadata.json").write_text("{}")
                (path / "capture.png").write_bytes(b"fixture pixels")
            # Glob sees these files before the Rust writer commits the directory.
            self.assertEqual(len(list(root.glob("*/metadata.json"))), 2)
            self.assertEqual(history_entries(root), set())
            staging.rename(committed)
            entries = history_entries(root)
            self.assertEqual(entries, {committed / "metadata.json"})
            self.assertEqual((next(iter(entries)).parent / "capture.png").read_bytes(), b"fixture pixels")

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
