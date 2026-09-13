from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import native_check
from src.native.recording import check as recording_check


class CaptureBoundsTests(unittest.TestCase):
    def test_pointer_bounds_map_window_coordinates_through_actual_client_origin(self):
        node = Mock()
        node.queryComponent().getExtents.return_value = SimpleNamespace(x=89, y=150, width=838, height=471)
        with patch.dict('sys.modules', {'pyatspi': SimpleNamespace(WINDOW_COORDS='window')}), \
             patch.object(native_check, 'cmd', return_value='401'), \
             patch.object(native_check, 'xwindow_geometry', return_value=(319, 155, 1280, 800)):
            rect = native_check.screen_bounds(node, 'Editor')
        node.queryComponent().getExtents.assert_called_once_with('window')
        self.assertEqual((rect.x, rect.y, rect.width, rect.height), (408, 305, 838, 471))

    def test_pointer_click_prefers_button_over_same_named_label_and_rejects_empty_bounds(self):
        for width in [158, 0]:
            with self.subTest(width=width), ExitStack() as stack:
                button = Mock()
                lookup = stack.enter_context(patch.object(native_check, 'find', return_value=button))
                bounds = stack.enter_context(patch.object(native_check, 'screen_bounds', return_value=(
                    SimpleNamespace(x=344, y=915, width=width, height=27))))
                command = stack.enter_context(patch.object(native_check, 'cmd'))
                stack.enter_context(patch.object(native_check.time, 'sleep'))
                if width:
                    native_check.click('Export settings', 'Editor', pointer=True)
                    command.assert_called_once_with('xdotool', 'mousemove', 423, 928, 'click', 1)
                else:
                    with self.assertRaises(AssertionError):
                        native_check.click('Export settings', 'Editor', pointer=True)
                    command.assert_not_called()
                lookup.assert_called_once_with('Export settings', role='push button', frame='Editor')
                bounds.assert_called_once_with(button, 'Editor')

    def test_root_crops_require_the_complete_client(self):
        # Include exact right/bottom edges, then one-pixel overflow on each
        # edge. A clipped capture must fail before ImageMagick is invoked.
        cases = [
            ((12, 9, 123, 77), True),
            ((37, 23, 123, 77), True),
            ((38, 23, 123, 77), False),
            ((37, 24, 123, 77), False),
            ((-1, 9, 123, 77), False),
            ((12, -1, 123, 77), False),
        ]
        for module, command_name, window in [
            (native_check, 'cmd', '401'),
            (recording_check, 'command', ['401']),
        ]:
            for geometry, valid in cases:
                with self.subTest(module=module.__name__, geometry=geometry), ExitStack() as stack:
                    stack.enter_context(patch.object(module, 'wait', return_value=window))
                    stack.enter_context(patch.object(module.time, 'sleep'))
                    stack.enter_context(patch.object(module, 'xwindow_geometry', return_value=geometry))
                    command = stack.enter_context(patch.object(
                        module, command_name, side_effect=['160 100', '', '123x77']))
                    if valid:
                        module.capture(Path('/unused'), 'fixture', 'Editor')
                        x, y, _, _ = geometry
                        self.assertEqual(command.call_args_list[1].args[:6], (
                            'import', '-window', 'root', '-crop', f'123x77+{x}+{y}', '+repage'))
                    else:
                        with self.assertRaisesRegex(AssertionError, 'exceeds desktop'):
                            module.capture(Path('/unused'), 'fixture', 'Editor')
                        command.assert_called_once_with('xdotool', 'getdisplaygeometry')

    def test_cropped_image_dimensions_are_checked_independently(self):
        for module, command_name, window in [
            (native_check, 'cmd', '401'),
            (recording_check, 'command', ['401']),
        ]:
            with self.subTest(module=module.__name__), ExitStack() as stack:
                stack.enter_context(patch.object(module, 'wait', return_value=window))
                stack.enter_context(patch.object(module.time, 'sleep'))
                stack.enter_context(patch.object(module, 'xwindow_geometry', return_value=(12, 9, 123, 77)))
                stack.enter_context(patch.object(module, command_name, side_effect=['160 100', '', '123x76']))
                with self.assertRaisesRegex(AssertionError, 'Clipped client capture'):
                    module.capture(Path('/unused'), 'fixture', 'Editor')


if __name__ == '__main__':
    unittest.main()
