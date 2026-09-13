#!/usr/bin/python3
"""Focused real-window check for the GTK image editor.

Run only against native_desktop.py's disposable Xvfb/DBus lab. Unlike the
whole-app native_check, canvas points stay inside the editor's narrower parity
layout with its persistent layer/properties sidebar.
"""
import argparse
import json
import os
import re
import subprocess
from contextlib import contextmanager
from pathlib import Path
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(json.loads((args.lab / 'environment.json').read_text()))

    # AT-SPI must be imported after the isolated session variables are active.
    from native_check import assert_button_ink_alignment, capture, choose, click, cmd, drag, find, ink_center, run, screen_bounds, wait, walk, xwindow_geometry

    root = Path(__file__).parent.resolve()
    binary = root / 'target/release/captures-linux-native'
    fixture = args.lab / 'fixture.png'
    args.artifacts.mkdir(parents=True, exist_ok=True)
    editor = 'Captures — Image editor'

    def canvas_bounds():
        # The canvas tooltip has the same accessible name but role=label and
        # a tiny text rectangle. Select the DrawingArea, not that popup text.
        canvas = wait(lambda: find('Screenshot editing canvas', role='filler', frame=editor))
        rect = screen_bounds(canvas, editor)
        assert rect.width >= 100 and rect.height >= 100, rect
        return rect

    def assert_canvas_metadata(width, height, format='PNG'):
        wait(lambda: find(f'{format} · {width} × {height}', 'label', editor))
        for name, expected in [('Canvas width', width), ('Canvas height', height)]:
            assert find(name, 'spin button', editor).queryValue().currentValue == expected

    with contextmanager(run)(binary, fixture, ['--open', str(fixture)], args.artifacts):
        import pyatspi
        wait(lambda: find(editor, 'frame'))
        assert_canvas_metadata(960, 540)
        for name, value in [('Canvas width', 957), ('Canvas height', 533)]:
            click(name, editor, pointer=True)
            cmd('xdotool', 'key', 'ctrl+a')
            cmd('xdotool', 'type', str(value))
            cmd('xdotool', 'key', 'Return')
        assert_canvas_metadata(957, 533)
        choose('Format', 1, editor)
        assert_canvas_metadata(957, 533, 'JPEG')
        click('Undo', editor)
        assert_canvas_metadata(957, 540, 'JPEG')
        click('Undo', editor)
        assert_canvas_metadata(960, 540, 'JPEG')
        click('Redo', editor)
        assert_canvas_metadata(957, 540, 'JPEG')
        click('Redo', editor)
        assert_canvas_metadata(957, 533, 'JPEG')
        choose('Format', 2, editor)
        assert_canvas_metadata(957, 533, 'WebP')
        for color in ('#193A7B', '#D87231'):
            click('Background color', editor, pointer=True)
            # Popover coordinates are relative to its native surface, not the
            # editor client. Tab from its initial switch focus into the entry.
            cmd('xdotool', 'key', 'Tab')
            wait(lambda: find('Canvas background hex value', 'text', editor)
                 .getState().contains(pyatspi.STATE_FOCUSED))
            cmd('xdotool', 'key', 'ctrl+a')
            cmd('xdotool', 'type', color)
            # Recommitting unchanged text must not consume another Undo step.
            cmd('xdotool', 'key', 'Return', 'Return')
            cmd('xdotool', 'key', 'Escape')
        click('Undo', editor)
        click('Background color', editor, pointer=True)
        text = find('Canvas background hex value', 'text', editor).queryText()
        assert text.getText(0, text.characterCount) == '#193A7B'
        cmd('xdotool', 'key', 'Escape')
        click('Undo', editor)
        click('Background color', editor, pointer=True)
        assert not find('Solid background', frame=editor).getState().contains(pyatspi.STATE_CHECKED)
        cmd('xdotool', 'key', 'Escape')
        click('Redo', editor)
        click('Background color', editor, pointer=True)
        assert find('Solid background', frame=editor).getState().contains(pyatspi.STATE_CHECKED)
        text = find('Canvas background hex value', 'text', editor).queryText()
        assert text.getText(0, text.characterCount) == '#193A7B'
        cmd('xdotool', 'key', 'Escape')
        capture(args.artifacts, 'editor-metadata-restored', editor)
        print('PASS live canvas/format metadata and background controls across exact undo/redo')

    with contextmanager(run)(binary, fixture, ['--open', str(fixture)], args.artifacts) as (_, output):
        wait(lambda: find(editor, 'frame'))
        window = cmd('xdotool', 'search', '--onlyvisible', '--name', editor).splitlines()[-1]
        cmd('xdotool', 'windowsize', window, '1280', '800')
        # Keep the managed window during interaction checks. GTK4 cannot report
        # screen coordinates for override-redirect windows, which invalidates
        # pointer-driven canvas and inline-edit assertions.
        cmd('xdotool', 'mousemove', 0, 0)
        capture(args.artifacts, 'editor-default-1280x800', editor)
        trims = [node for node in walk(find(editor, 'frame'))
                 if node.name == 'Trim edges' and node.getRoleName() == 'push button']
        assert len(trims) == 1, 'The editor must have only one Trim action'
        assert find('1 layer', 'label', editor)
        assert not find('Layers', 'label', editor)
        for dimension in ('Canvas width', 'Canvas height'):
            field = find(dimension, 'spin button', editor)
            assert field and not any(child.getRoleName() == 'push button' for child in walk(field)), dimension
        copy_switch = screen_bounds(find('Save as new file', frame=editor), editor)
        assert copy_switch.width <= 30 and copy_switch.height <= 18, copy_switch
        # Scoped editor CSS must not override the shared primary hover treatment.
        # Exercise pointer hover without exporting, and check a text-free pixel.
        save = wait(lambda: find('Save', frame=editor))
        bounds = screen_bounds(save, editor)
        copy_bounds = screen_bounds(find('Copy image', frame=editor), editor)
        filename_bounds = screen_bounds(find('Filename', role='text', frame=editor), editor)
        assert bounds.height == copy_bounds.height == 36, (bounds, copy_bounds)
        assert abs(bounds.y - copy_bounds.y) <= 1, (bounds, copy_bounds)
        assert abs(bounds.y + bounds.height - filename_bounds.y - filename_bounds.height) <= 1
        assert abs(copy_switch.y + copy_switch.height - bounds.y - bounds.height) <= 1, (copy_switch, bounds)
        client_x, client_y, _, _ = xwindow_geometry(window)
        shot = args.artifacts / 'after-editor-default-1280x800.png'
        origin = (client_x, client_y)
        for name in ('Save', 'Copy image', 'Trim edges', 'Add images'):
            assert_button_ink_alignment(find(name, 'push button', editor), editor, shot, origin)
        toggle_text = ink_center(find('Save as new file', 'label', editor), editor, shot, origin)
        assert abs(toggle_text - copy_switch.y - (copy_switch.height - 1) / 2) <= .5, toggle_text
        background = find('Background color', 'push button', editor)
        swatch, label, chevron = [node for node in walk(background)
                                   if node.getRoleName() in ('filler', 'label')]
        swatch_rect = screen_bounds(swatch, editor)
        button_rect = screen_bounds(background, editor)
        chevron_rect = screen_bounds(chevron, editor)
        assert swatch_rect.x - button_rect.x >= 12
        assert button_rect.x + button_rect.width - chevron_rect.x - chevron_rect.width >= 12
        assert abs(ink_center(label, editor, shot, origin) - swatch_rect.y - (swatch_rect.height - 1) / 2) <= .5
        cmd('xdotool', 'mousemove', bounds.x + bounds.width // 2, bounds.y + bounds.height // 2)
        capture(args.artifacts, 'editor-save-hover', editor)
        sample = f'%[pixel:p{{{bounds.x - client_x + 10},{bounds.y - client_y + 10}}}]'
        idle, hovered = [cmd('convert', args.artifacts / f'after-{state}.png', '-format', sample, 'info:')
                         for state in ['editor-default-1280x800', 'editor-save-hover']]
        assert idle.startswith('srgb') and hovered.startswith('srgb')
        assert idle != hovered, (idle, hovered)
        cmd('xdotool', 'mousemove', 0, 0)
        area = canvas_bounds()
        # At 100%, the canvas exceeds the viewport. Space-drag must pan it.
        cmd('xdotool', 'mousemove', area.x + 200, area.y + 150, 'click', 1, 'key', 'ctrl+0')
        full_size = canvas_bounds()
        cmd('xdotool', 'keydown', 'space')
        drag(full_size.x + 300, full_size.y + 180, -100, 0)
        cmd('xdotool', 'keyup', 'space')
        panned = canvas_bounds()
        assert panned.x < full_size.x and panned.width == full_size.width, (full_size, panned)
        click('Fit', editor)
        area = canvas_bounds()
        click('Arrow (A)', editor)
        drag(area.x + 90, area.y + 90, 220, 130)
        wait(lambda: find('2 layers', 'label', editor))
        arrow = wait(lambda: next((layer
            for draft in (output.parent / 'editor-drafts').glob('*.json')
            for layer in json.loads(draft.read_text())['layers']
            if 'Arrow' in layer['kind']), None))
        # Derive source geometry from the displayed fixture allocation, not
        # from the event adapter. Allow subpixel allocation rounding only.
        for key, screen_distance in [('x', 90), ('y', 90), ('w', 220), ('h', 130)]:
            expected = screen_distance * 960 / area.width
            assert abs(arrow['frame'][key] - expected) < .5, (key, arrow['frame'][key], expected)
        # Every layer has this label; choose the entry in the Arrow's row,
        # rather than accidentally renaming the locked background.
        rename = wait(lambda: next((node for node in walk(find(editor, 'frame'))
                                   if node.getRoleName() == 'text' and node.name == 'Rename layer'
                                   and any(child.name == 'Arrow' and child.getRoleName() == 'label'
                                           for child in node.parent)), None))
        rename_bounds = screen_bounds(rename, editor)
        cmd('xdotool', 'mousemove', rename_bounds.x + rename_bounds.width // 2,
            rename_bounds.y + rename_bounds.height // 2, 'click', 1)
        cmd('xdotool', 'key', 'ctrl+a')
        cmd('xdotool', 'type', 'Callout arrow')
        cmd('xdotool', 'key', 'Return')
        wait(lambda: any(
            layer['name'] == 'Callout arrow'
            for draft in (output.parent / 'editor-drafts').glob('*.json')
            for layer in json.loads(draft.read_text())['layers']
        ))

        click('Remove background (B)', editor)
        wait(lambda: find('Color tolerance', role='slider', frame=editor))
        wait(lambda: find('Contiguous only', role='check box', frame=editor))
        capture(args.artifacts, 'editor-remove-background', editor)
        click('Erase', editor)
        wait(lambda: find('Brush softness', role='slider', frame=editor))

        click('Shapes', editor)
        click('Triangle', editor)
        # Let the native popover finish closing before sending canvas input.
        time.sleep(.3)
        area = canvas_bounds()
        drag(area.x + 370, area.y + 90, 70, 90)
        wait(lambda: any(
            'Triangle' in layer['kind']
            for draft in (output.parent / 'editor-drafts').glob('*.json')
            for layer in json.loads(draft.read_text())['layers']
        ))
        click('Select & move (V)', editor)
        cmd('xdotool', 'mousemove', area.x + 405, area.y + 135, 'click', 1,
            'key', 'ctrl+c', 'key', 'ctrl+v')
        wait(lambda: any(
            'Triangle copy' in draft.read_text()
            for draft in (output.parent / 'editor-drafts').glob('*.json')
        ))
        click('Undo', editor)
        area = canvas_bounds()
        cmd('xdotool', 'mousemove', area.x + 405, area.y + 135, 'click', 1)
        capture(args.artifacts, 'editor-selected-layer', editor)

        # Selected-layer style edits must affect the document, not just the
        # drawing defaults. Undo restores the original color in one action.
        def triangle():
            return next(layer for draft in (output.parent / 'editor-drafts').glob('*.json')
                        for layer in json.loads(draft.read_text())['layers']
                        if layer['name'] == 'Triangle')
        original_color = triangle()['color']
        color = wait(lambda: find('Stroke color hex value', role='text', frame=editor))
        color.queryEditableText().setTextContents('#2563EB')
        color_bounds = screen_bounds(color, editor)
        cmd('xdotool', 'mousemove', color_bounds.x + 35, color_bounds.y + 12,
            'click', 1, 'key', 'Return')
        wait(lambda: triangle()['color'] == [37 / 255, 99 / 255, 235 / 255, 1])
        click('Undo', editor)
        assert triangle()['color'] == original_color
        cmd('xdotool', 'mousemove', area.x + 405, area.y + 135, 'click', 1)
        click('Layer settings for Triangle', editor, pointer=True)
        wait(lambda: find('Duplicate', role='push button', frame=editor))
        capture(args.artifacts, 'editor-layer-menu', editor)
        click('Duplicate', editor)
        wait(lambda: any('Triangle copy' in draft.read_text()
                         for draft in (output.parent / 'editor-drafts').glob('*.json')))
        click('Undo', editor)
        assert not any('Triangle copy' in draft.read_text()
                       for draft in (output.parent / 'editor-drafts').glob('*.json'))

        click('Export settings', editor, pointer=True)
        time.sleep(.4)
        assert find('Compression comparison slider', 'slider', editor)
        capture(args.artifacts, 'editor-export-settings', editor)

        choose('Save quality', 2, editor)
        maximum_unit = wait(lambda: find('Maximum file size unit', 'combo box', editor))
        # GTK4 can clear SHOWING on mapped X11 controls. Require a real
        # allocation and removal from the tree when the field is hidden.
        assert screen_bounds(maximum_unit, editor).width > 0
        capture(args.artifacts, 'editor-maximum-size', editor)
        choose('Save quality', 0, editor)
        assert find('Maximum file size unit', 'combo box', editor) is None
        click('Export settings', editor, pointer=True)

        click('Save as new file', editor)
        click('Save', editor)
        first = wait(lambda: next(output.glob('*.png'), None))
        assert cmd('identify', '-format', '%wx%h', first) == '960x540'

        click('Undo', editor)
        click('Undo', editor)
        click('Undo', editor)
        click('Save', editor)
        wait(lambda: len(list(output.glob('*.png'))) == 2)
        clean = sorted(output.glob('*.png'))[-1]
        assert cmd('compare', '-metric', 'AE', fixture, clean, 'null:') == ''

        click('Redo', editor)
        click('Redo', editor)
        click('Redo', editor)
        click('Crop (C)', editor)
        area = canvas_bounds()
        # Keep both endpoints in the visible portion of the initial fit-to-window canvas.
        drag(area.x + 330, area.y + 220, -200, -120)
        click('Save', editor)
        wait(lambda: len(list(output.glob('*.png'))) == 3)
        cropped = sorted(output.glob('*.png'))[-1]
        cropped_size = cmd('identify', '-format', '%wx%h', cropped)
        cropped_width, cropped_height = map(int, cropped_size.split('x'))
        assert 0 < cropped_width < 960 and 0 < cropped_height < 540
        assert_canvas_metadata(cropped_width, cropped_height)
        click('Undo', editor)
        assert_canvas_metadata(960, 540)
        click('Redo', editor)
        assert_canvas_metadata(cropped_width, cropped_height)

        choose('Format', 1, editor)
        assert_canvas_metadata(cropped_width, cropped_height, 'JPEG')
        capture(args.artifacts, 'editor-cropped-jpeg', editor)
        click('Save', editor)
        jpeg = wait(lambda: next(output.glob('*.jpg'), None))
        assert cmd('identify', '-format', '%m %wx%h', jpeg) == f'JPEG {cropped_size}'

    # Source identity must survive open_file integration. Save overwrites only
    # this disposable source; closing/reopening first proves draft restoration.
    with tempfile.TemporaryDirectory(prefix='captures-source-save-') as temporary:
        source = Path(temporary) / 'source.png'
        original = fixture.read_bytes()
        source.write_bytes(original)
        with contextmanager(run)(binary, source, ['--open', str(source)], args.artifacts) as (_, output):
            wait(lambda: find(editor, 'frame'))
            area = canvas_bounds()
            click('Shapes', editor)
            click('Rectangle', editor)
            drag(area.x + 70, area.y + 60, 150, 100)
            wait(lambda: list((output.parent / 'editor-drafts').glob('*.json')))
            window = cmd('xdotool', 'search', '--onlyvisible', '--name', editor).splitlines()[-1]
            cmd('xdotool', 'windowactivate', '--sync', window, 'key', 'alt+F4')
            subprocess.run([str(binary), '--open', str(source)], check=True, timeout=8)
            wait(lambda: find('Unsaved editing draft restored — export, save, or keep editing.', frame=editor))
            assert source.read_bytes() == original
            click('Save', editor)
            wait(lambda: source.read_bytes() != original)
            wait(lambda: not list((output.parent / 'editor-drafts').glob('*.json')))
            assert cmd('identify', '-format', '%wx%h', source) == '960x540'
            assert not list(output.glob('*.png')), 'Save must not create an unrelated export'

        transparent = Path(temporary) / 'transparent.png'
        cmd('convert', '-size', '240x160', 'xc:none', transparent)
        with contextmanager(run)(binary, transparent, ['--open', str(transparent)], args.artifacts) as (_, output):
            wait(lambda: find(editor, 'frame'))
            area = canvas_bounds()
            cmd('xdotool', 'mousemove', area.x + 30, area.y + 30, 'click', 1, 'key', 'ctrl+0')
            capture(args.artifacts, 'canvas', editor)
            area = canvas_bounds()
            shot = Path(temporary) / 'screen.png'
            cmd('import', '-window', 'root', shot)
            pixel = cmd('convert', shot, '-format',
                        f'%[pixel:p{{{area.x + 8},{area.y + 8}}}]', 'info:')
            channels = re.fullmatch(r'srgb\((\d+),(\d+),(\d+)\)', pixel)
            # Compare every channel to the light document surface (#f7f7f5),
            # allowing one 8-bit level for Cairo/compositor quantization.
            assert channels and all(abs(int(actual) - expected) <= 1
                                    for actual, expected in zip(channels.groups(), (247, 247, 245))), (
                'Transparent canvas should use the shipping light document surface for display', pixel
            )
            click('Save as new file', editor)
            click('Save', editor)
            exported = wait(lambda: next(output.glob('*.png'), None))
            assert cmd('identify', '-format', '%[opaque]', exported) == 'false'

    print('PASS editor: space-pan, shape flyout/drawing, inline rename, layer copy/paste, wand/soft brush controls, compression/maximum-size controls, exact undo, reverse crop, PNG/JPEG export')
    print('PASS source Save: unchanged source before Save, automatic draft reopen, source overwrite, draft removal')


if __name__ == '__main__':
    main()
