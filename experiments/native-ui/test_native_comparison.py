import tempfile
import unittest
from pathlib import Path
from subprocess import CompletedProcess
from unittest.mock import patch

import native_comparison as comparison


class ComparisonTests(unittest.TestCase):
    def test_window_lookup_requires_both_pid_and_title(self):
        with patch.object(comparison.subprocess, 'run', return_value=CompletedProcess([], 0, '401\n')) as run:
            self.assertEqual(comparison.visible('Preferences', 123), '401')
        self.assertEqual(run.call_args.args[0], [
            'xdotool', 'search', '--onlyvisible', '--all', '--pid', '123', '--name', 'Preferences',
        ])

    def test_capture_reads_compositor_at_asymmetric_client_bounds(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'capture.png'
            target.write_bytes(b'\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR'
                               + (123).to_bytes(4, 'big') + (77).to_bytes(4, 'big'))
            with patch.object(comparison.subprocess, 'check_output', side_effect=['X=12\nY=9\nWIDTH=123\nHEIGHT=77\n', '2']), \
                 patch.object(comparison.subprocess, 'run') as run:
                comparison.capture_window('401', target, (123, 77))
            self.assertEqual(run.call_args.args[0], [
                'import', '-window', 'root', '-crop', '123x77+12+9', str(target),
            ])

    def test_clamped_window_size_is_not_reported_as_matched(self):
        with patch.object(comparison.subprocess, 'check_output', return_value='X=0\nY=0\nWIDTH=125\nHEIGHT=77\n'), \
             patch.object(comparison.subprocess, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'Window viewport'):
                comparison.capture_window('401', Path('unused.png'), (123, 77))
            run.assert_not_called()

    def test_clipped_compositor_capture_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'capture.png'
            target.write_bytes(b'\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR'
                               + (100).to_bytes(4, 'big') + (77).to_bytes(4, 'big'))
            with patch.object(comparison.subprocess, 'check_output', return_value='X=12\nY=9\nWIDTH=123\nHEIGHT=77\n'), \
                 patch.object(comparison.subprocess, 'run'), \
                 self.assertRaisesRegex(RuntimeError, 'Compositor capture'):
                comparison.capture_window('401', target, (123, 77))

    def test_correctly_sized_blank_gpu_surface_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'capture.png'
            target.write_bytes(b'\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR'
                               + (123).to_bytes(4, 'big') + (77).to_bytes(4, 'big'))
            with patch.object(comparison.subprocess, 'check_output', side_effect=['X=0\nY=0\nWIDTH=123\nHEIGHT=77\n', '1']), \
                 patch.object(comparison.subprocess, 'run'), \
                 self.assertRaisesRegex(RuntimeError, 'blank'):
                comparison.capture_window('401', target, (123, 77))


if __name__ == '__main__':
    unittest.main()
