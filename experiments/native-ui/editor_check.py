#!/usr/bin/python3
"""Focused real-window check for the GTK image editor.

Run only against native_desktop.py's disposable Xvfb/DBus lab. Unlike the
whole-app native_check, canvas points stay inside the editor's narrower parity
layout with its persistent layer/properties sidebar.
"""
import argparse
import json
import os
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
    import pyatspi
    from native_check import capture, choose, click, cmd, drag, find, run, wait

    root = Path(__file__).parent.resolve()
    binary = root / 'target/release/captures-linux-native'
    fixture = args.lab / 'fixture.png'
    args.artifacts.mkdir(parents=True, exist_ok=True)
    editor = 'Captures — Image editor'

    with contextmanager(run)(binary, fixture, ['--open', str(fixture)], args.artifacts) as (_, output):
        wait(lambda: find(editor, 'frame'))
        window = cmd('xdotool', 'search', '--onlyvisible', '--name', editor).splitlines()[-1]
        cmd('xdotool', 'windowsize', window, '1280', '800')
        # Openbox keeps decorated 1280px-wide windows below its title-bar area,
        # which clips the bottom 20px in a 1280x800 Xvfb screenshot. The Tauri
        # reference is a client-area capture, so remove lab-only decorations and
        # anchor the native client at the same exact viewport before comparing.
        cmd('xdotool', 'set_window', '--overrideredirect', '1', window)
        cmd('xdotool', 'windowmove', window, '0', '0')
        cmd('xdotool', 'mousemove', 0, 0)
        capture(args.artifacts, 'editor-default-1280x800', editor)
        # Scoped editor CSS must not override the shared primary hover treatment.
        # Exercise pointer hover without exporting, and check a text-free pixel.
        save = find('Save', frame=editor)
        bounds = save.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
        cmd('xdotool', 'mousemove', bounds.x + bounds.width // 2, bounds.y + bounds.height // 2)
        capture(args.artifacts, 'editor-save-hover', editor)
        sample = f'%[pixel:p{{{bounds.x + 10},{bounds.y + 10}}}]'
        idle, hovered = [cmd('convert', args.artifacts / f'after-{state}.png', '-format', sample, 'info:')
                         for state in ['editor-default-1280x800', 'editor-save-hover']]
        assert idle != hovered, ('Save hover did not change its painted surface', idle, hovered)
        cmd('xdotool', 'mousemove', 0, 0)
        save.queryComponent().grabFocus()
        assert save.getState().contains(pyatspi.STATE_FOCUSED)
        capture(args.artifacts, 'editor-save-focus', editor)
        area = find(role='drawing area', frame=editor).queryComponent().getExtents(
            pyatspi.DESKTOP_COORDS
        )
        # At 100%, the canvas exceeds the viewport. Space-drag must pan it.
        cmd('xdotool', 'mousemove', area.x + 200, area.y + 150, 'click', 1, 'key', 'ctrl+0')
        full_size = find(role='drawing area', frame=editor).queryComponent().getExtents(
            pyatspi.DESKTOP_COORDS
        )
        cmd('xdotool', 'keydown', 'space')
        drag(full_size.x + 300, full_size.y + 180, -100, 0)
        cmd('xdotool', 'keyup', 'space')
        panned = find(role='drawing area', frame=editor).queryComponent().getExtents(
            pyatspi.DESKTOP_COORDS
        )
        assert panned.x < full_size.x
        click('Fit', editor)
        area = find(role='drawing area', frame=editor).queryComponent().getExtents(
            pyatspi.DESKTOP_COORDS
        )
        click('Arrow (A)', editor)
        drag(area.x + 90, area.y + 90, 220, 130)
        rename = find('Rename layer', frame=editor)
        rename.queryComponent().grabFocus()
        rename.queryEditableText().setTextContents('Callout arrow')
        cmd('xdotool', 'key', 'Return')
        wait(lambda: any(
            'Callout arrow' in draft.read_text()
            for draft in (output.parent / 'editor-drafts').glob('*.json')
        ))

        click('Remove background (B)', editor)
        assert find('Color tolerance', frame=editor)
        assert find('Contiguous only', frame=editor)
        capture(args.artifacts, 'editor-remove-background', editor)
        click('Erase', editor)
        assert find('Brush softness', frame=editor)

        click('Shapes', editor)
        click('Triangle', editor)
        drag(area.x + 370, area.y + 90, 70, 90)
        click('Select & move (V)', editor)
        cmd('xdotool', 'mousemove', area.x + 405, area.y + 135, 'click', 1,
            'key', 'ctrl+c', 'key', 'ctrl+v')
        wait(lambda: any(
            'Triangle copy' in draft.read_text()
            for draft in (output.parent / 'editor-drafts').glob('*.json')
        ))
        click('Undo', editor)
        selected_name = find('Rename layer', frame=editor)
        selected_name.queryComponent().grabFocus()
        cmd('xdotool', 'key', 'Return')
        capture(args.artifacts, 'editor-selected-layer', editor)

        click('Export settings', editor)
        time.sleep(.4)
        capture(args.artifacts, 'editor-export-settings', editor)
        click('Compare', editor, pointer=True)
        wait(lambda: find('Compression comparison', 'dialog'))
        assert find('Before', frame='Compression comparison')
        assert find('After', frame='Compression comparison')
        click('Close', 'Compression comparison')

        choose('Preserve quality', 2, editor)
        assert find('Maximum file size', 'combo box', editor)
        assert wait(lambda: find('MB', 'combo box', editor))
        capture(args.artifacts, 'editor-maximum-size', editor)
        click('Compare', editor, pointer=True)
        wait(lambda: find('Compression comparison', 'dialog'))
        assert find('Before', frame='Compression comparison')
        assert find('After', frame='Compression comparison')
        click('Close', 'Compression comparison')
        choose('Maximum file size', 0, editor)
        click('Export settings', editor)

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
        area = find(role='drawing area', frame=editor).queryComponent().getExtents(
            pyatspi.DESKTOP_COORDS
        )
        # Keep both endpoints in the visible portion of the initial fit-to-window canvas.
        drag(area.x + 330, area.y + 220, -200, -120)
        click('Save', editor)
        wait(lambda: len(list(output.glob('*.png'))) == 3)
        cropped = sorted(output.glob('*.png'))[-1]
        cropped_size = cmd('identify', '-format', '%wx%h', cropped)
        cropped_width, cropped_height = map(int, cropped_size.split('x'))
        assert 0 < cropped_width < 960 and 0 < cropped_height < 540

        choose('PNG', 1, editor)
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
            area = find(role='drawing area', frame=editor).queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            click('Shapes', editor)
            click('Rectangle', editor)
            drag(area.x + 70, area.y + 60, 150, 100)
            wait(lambda: list((output.parent / 'editor-drafts').glob('*.json')))
            window = cmd('xdotool', 'search', '--onlyvisible', '--name', editor).splitlines()[-1]
            cmd('xdotool', 'windowactivate', '--sync', window, 'key', 'alt+F4')
            import subprocess
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
            area = find(role='drawing area', frame=editor).queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            cmd('xdotool', 'mousemove', area.x + 30, area.y + 30, 'click', 1, 'key', 'ctrl+0')
            capture(args.artifacts, 'canvas', editor)
            area = find(role='drawing area', frame=editor).queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            shot = Path(temporary) / 'screen.png'
            cmd('import', '-window', 'root', shot)
            colors = cmd('convert', shot, '-crop', f'24x12+{area.x}+{area.y}', '+repage', '-format', '%k', 'info:')
            assert int(colors) >= 2, 'Transparent canvas must display alternating checker cells, not black'
            click('Save as new file', editor)
            click('Save', editor)
            exported = wait(lambda: next(output.glob('*.png'), None))
            assert cmd('identify', '-format', '%[opaque]', exported) == 'false'

    print('PASS editor: space-pan, shape flyout/drawing, inline rename, layer copy/paste, wand/soft brush controls, compression/maximum-size controls, exact undo, reverse crop, PNG/JPEG export')
    print('PASS source Save: unchanged source before Save, automatic draft reopen, source overwrite, draft removal')


if __name__ == '__main__':
    main()
