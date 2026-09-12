#!/usr/bin/python3
"""Native parity regression checks on the disposable X11/DBus desktop, using real input."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from benchmark import stop
from native_check import cmd, walk, find, wait, click, drag, capture, choose


def close_preferences():
    window = cmd('xdotool', 'search', '--onlyvisible', '--name', '^Captures Preferences$').splitlines()[-1]
    cmd('xdotool', 'windowactivate', '--sync', window, 'key', 'alt+F4')
    wait(lambda: not find('Captures Preferences', 'frame'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(json.loads((args.lab / 'environment.json').read_text()))
    import pyatspi
    import dbus
    saver = dbus.Interface(dbus.SessionBus().get_object('org.freedesktop.ScreenSaver',
                          '/org/freedesktop/ScreenSaver'), 'org.freedesktop.ScreenSaver')
    args.artifacts.mkdir(parents=True, exist_ok=True)
    binary = Path(__file__).resolve().parent / 'target/release/captures-linux-native'
    with tempfile.TemporaryDirectory(prefix='captures-parity-') as temp:
        profile = Path(temp)
        env = dict(os.environ, CAPTURES_NATIVE_DATA=str(profile), XDG_CONFIG_HOME=str(profile/'config'))
        # Disable pointer/auto-copy only where pixel comparison requires it.
        # An earlier native build allowed this unsupported preference. Loading
        # it must recover to MP4 without losing the rest of the user's settings.
        (profile/'settings.json').write_text(json.dumps(dict(output_directory=str(profile/'captures'), show_cursor_in_screenshots=False, recording=dict(countdown_seconds=0, video_format='webm', video_fps=24))))
        with (profile/'app.log').open('w+') as log:
            process = subprocess.Popen([str(binary), '--capture'], env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                selector = 'Captures — Select target'
                wait(lambda: find(selector, 'frame'))
                assert not find('Captures — Linux native', 'frame'), 'No extra launcher screen'
                assert not find('Frames per second', frame=selector), 'Recording options hidden for screenshots'
                capture(args.artifacts, 'capture-menu', selector)
                click('Record', selector)
                wait(lambda: find('Frames per second', frame=selector))
                capture(args.artifacts, 'recording-selector', selector)
                click('Screenshot', selector)
                click('Screenshot', selector)  # An already selected segment must stay selected, without recursion.
                assert not find('Frames per second', frame=selector)
                cmd('xdotool', 'mousemove', 200, 240, 'mousedown', 1,
                    'mousemove', 600, 450)
                time.sleep(.2)
                cmd('import', '-window', 'root', profile/'free.png')
                cmd('xdotool', 'keydown', 'Shift_L')
                time.sleep(.2)
                cmd('import', '-window', 'root', profile/'square.png')
                cmd('xdotool', 'keyup', 'Shift_L')
                time.sleep(.2)
                cmd('import', '-window', 'root', profile/'released.png')
                cmd('xdotool', 'mouseup', 1)
                # This point is inside the 400x400 square but outside the
                # 400x210 free rectangle. No pointer motion follows Shift.
                pixels = [cmd('convert', profile/f'{name}.png', '-format',
                              '%[pixel:p{400,550}]', 'info:')
                          for name in ['free', 'square', 'released']]
                assert pixels[0] != pixels[1] and pixels[0] == pixels[2], pixels
                choose('Selection aspect ratio', 1, selector)
                capture(args.artifacts, 'region-square', selector)
                # The same preset as Tauri centers an inscribed 210x210 square.
                click('Capture', selector)
                square = wait(lambda: next((profile/'captures').glob('*.png'), None))
                assert cmd('identify', '-format', '%wx%h', square) == '210x210'
                # This first artifact intentionally stays out of the independent
                # capture/history-count scenarios below.
                wait(lambda: (profile/'history.json').exists() and len(json.loads((profile/'history.json').read_text())) == 1)
                click('Expand preview')
                preview = wait(lambda: find(f'Preview {square.name}'))
                bounds = preview.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
                cmd('xdotool', 'mousemove', bounds.x + bounds.width//2, bounds.y + bounds.height//2)
                click(f'Close {square.name}')
                square.unlink()
                (profile/'history.json').write_text('[]')
                cmd('xdotool', 'key', 'Print')
                wait(lambda: find(selector, 'frame'))
                drag(710, 460, -380, -220)
                try:
                    saver.SetActive(True)
                    click('Capture', selector)
                    click('Close')
                    wait(lambda: not find(selector, 'frame'))
                    assert not list((profile/'captures').glob('*.png')), 'Capture published while locked'
                finally:
                    saver.SetActive(False)
                cmd('xdotool', 'key', 'Print')
                wait(lambda: find(selector, 'frame'))
                selector_window = cmd('xdotool', 'search', '--onlyvisible', '--name', selector).splitlines()[-1]
                cmd('xdotool', 'windowactivate', '--sync', selector_window, 'key', 'Escape')
                wait(lambda: not find(selector, 'frame'))
                print('PASS session: lock during selection prevents publication; selector retry works', flush=True)
                subprocess.run([str(binary), '--preferences'], env=env, check=True, timeout=8)
                wait(lambda: find('Captures Preferences', 'frame'))
                preferences_window = cmd('xdotool', 'search', '--onlyvisible', '--name', '^Captures Preferences$')
                subprocess.run([str(binary), '--preferences'], env=env, check=True, timeout=8)
                assert cmd('xdotool', 'search', '--onlyvisible', '--name', '^Captures Preferences$') == preferences_window
                capture(args.artifacts, 'preferences-general', 'Captures Preferences')
                click('Appearance preferences section', 'Captures Preferences')
                assert find('Appearance preferences section', frame='Captures Preferences').getState().contains(pyatspi.STATE_CHECKED)
                capture(args.artifacts, 'preferences-appearance', 'Captures Preferences')
                choose('System', 2, 'Captures Preferences')
                cobalt = 'Cobalt: True blue and coral'
                mustard = 'Mustard: Captures mustard and signal red'
                custom = 'Custom: Build your own RGB palette'
                click(cobalt, 'Captures Preferences', pointer=True)
                wait(lambda: find(cobalt, frame='Captures Preferences').getState().contains(pyatspi.STATE_CHECKED))
                assert not find(mustard, frame='Captures Preferences').getState().contains(pyatspi.STATE_CHECKED)
                wait(lambda: (value := json.loads((profile/'settings.json').read_text()))['appearance']=='dark' and value['theme']=='cobalt')
                assert not find('Save changes', frame='Captures Preferences')
                capture(args.artifacts, 'preferences-dark', 'Captures Preferences')
                click(custom, 'Captures Preferences', pointer=True)
                wait(lambda: find(custom, frame='Captures Preferences').getState().contains(pyatspi.STATE_CHECKED))
                assert find('Custom accent', frame='Captures Preferences')
                wait(lambda: json.loads((profile/'settings.json').read_text())['theme']=='custom')
                capture(args.artifacts, 'preferences-dark-custom', 'Captures Preferences')
                choose('Dark', 1, 'Captures Preferences')
                click(mustard, 'Captures Preferences', pointer=True)
                wait(lambda: (value := json.loads((profile/'settings.json').read_text()))['appearance']=='light' and value['theme']=='mustard')
                click('Recording preferences section', 'Captures Preferences')
                rates = find('30 fps', 'combo box', 'Captures Preferences')
                assert rates, 'Legacy 24 fps must migrate to a supported video rate'
                assert [node.name for node in walk(rates) if node.getRoleName() == 'menu item'] == ['15 fps', '30 fps', '60 fps']
                choose('30 fps', 0, 'Captures Preferences')
                wait(lambda: json.loads((profile/'settings.json').read_text())['recording']['video_fps']==15)
                formats = find('MP4', 'combo box', 'Captures Preferences')
                assert formats, 'Legacy WebM must load as MP4'
                assert [node.name for node in walk(formats) if node.getRoleName() == 'menu item'] == ['MP4', 'GIF'], 'Only supported recording formats may be offered'
                choose('MP4', 1, 'Captures Preferences')
                assert find('GIF', 'combo box', 'Captures Preferences')
                wait(lambda: json.loads((profile/'settings.json').read_text())['recording']['video_format']=='gif')
                capture(args.artifacts, 'preferences-recording-gif', 'Captures Preferences')
                choose('GIF', 0, 'Captures Preferences')
                wait(lambda: json.loads((profile/'settings.json').read_text())['recording']['video_format']=='mp4')
                print('PASS recording defaults: legacy WebM/24 fps migrate; only MP4/GIF and 15/30/60 fps selectable and persisted', flush=True)
                capture(args.artifacts, 'preferences-recording', 'Captures Preferences')
                close_preferences()
                # A second invocation must route to the running application and exit.
                subprocess.run([str(binary), '--preferences'], env=env, check=True, timeout=8)
                wait(lambda: find('Captures Preferences', 'frame'))
                close_preferences()
                # Capture an independent ground truth with all Captures windows hidden.
                cmd('import', '-window', 'root', profile/'reference.png')
                # A successful capture must restore a regular window hidden for
                # capture, not just handle the cancellation path.
                subprocess.run([str(binary), '--preferences'], env=env, check=True, timeout=8)
                wait(lambda: find('Captures Preferences', 'frame'))
                cmd('xdotool', 'key', 'Print')
                wait(lambda: find('Captures — Select target', 'frame'))
                drag(180, 130, 320, 170)
                capture(args.artifacts, 'region', 'Captures — Select target')
                # Move the existing selection, then resize its lower-right corner.
                drag(250, 180, 40, 30)
                drag(540, 330, 60, 20)
                capture(args.artifacts, 'region-resized', 'Captures — Select target')
                click('Capture', 'Captures — Select target')
                path = wait(lambda: next((profile/'captures').glob('*.png'), None))
                assert cmd('identify', '-format', '%wx%h', path) == '380x190'
                cmd('convert', profile/'reference.png', '-crop', '380x190+220+160', '+repage', profile/'expected.png')
                subprocess.run(['compare','-metric','AE',str(profile/'expected.png'),str(path),'null:'],check=True)
                wait(lambda: find('Captures Preferences', 'frame'))
                close_preferences()
                print('PASS region capture, move, resize: 380x190', flush=True)
                # Global X11 binding must work with the menu hidden.
                cmd('xdotool', 'key', 'Print')
                wait(lambda: find('Captures — Select target', 'frame'))
                click('Window', 'Captures — Select target')
                cmd('xdotool', 'mousemove', '240', '180')
                time.sleep(.3)
                capture(args.artifacts, 'window', 'Captures — Select target')
                selector_window = cmd('xdotool', 'search', '--onlyvisible', '--name', selector).splitlines()[-1]
                cmd('xdotool', 'windowactivate', '--sync', selector_window, 'key', 'Escape')
                wait(lambda: not find(selector, 'frame'))
                subprocess.run([str(binary), '--history'], env=env, check=True, timeout=8)
                wait(lambda: find('Captures — History', 'frame'))
                wait(lambda: find(f'Edit {path.name}', frame='Captures — History'))
                capture(args.artifacts, 'history', 'Captures — History')
                assert len(json.loads((profile/'history.json').read_text())) == 1
                cmd('xdotool', 'key', 'Print')
                wait(lambda: find('Captures — Select target', 'frame'))
                click('Window', 'Captures — Select target')
                cmd('xdotool', 'mousemove', '650', '300')
                time.sleep(.3)
                capture(args.artifacts, 'window', 'Captures — Select target')
                click('Capture', 'Captures — Select target')
                wait(lambda: len(list((profile/'captures').glob('*.png'))) == 2)
                window_capture = sorted((profile/'captures').glob('*.png'))[-1]
                w, h = map(int, cmd('identify', '-format', '%wx%h', window_capture).split('x'))
                assert 900 < w < 1280 and 500 < h < 800, (w, h)
                cmd('xdotool', 'key', 'Print')
                wait(lambda: find('Captures — Select target', 'frame'))
                click('Full screen', 'Captures — Select target')
                capture(args.artifacts, 'display', 'Captures — Select target')
                click('Capture', 'Captures — Select target')
                wait(lambda: len(list((profile/'captures').glob('*.png'))) == 3)
                display_capture = sorted((profile/'captures').glob('*.png'))[-1]
                assert cmd('identify', '-format', '%wx%h', display_capture) == '1280x800'
                print('PASS window capture and 1280x800 full-display capture', flush=True)
                print('PASS background hotkey, cancellation, single-instance routing, saved history', flush=True)
                log.flush(); log.seek(0)
                output = log.read()
                assert 'Native theme:' not in output, output
                assert 'panicked' not in output, output
                print('PASS shared light palette and CSS load; Preferences sections rendered', flush=True)
            except Exception:
                log.flush(); log.seek(0); print(log.read(), flush=True)
                for app in pyatspi.Registry.getDesktop(0):
                    print('APP', app.name, flush=True)
                    if 'captures' in app.name.lower():
                        for node in walk(app):
                            if node.name:
                                print(node.getRoleName(), node.name, flush=True)
                cmd('import', '-window', 'root', '/tmp/captures-parity-failure.png')
                raise
            finally:
                stop(process)


if __name__ == '__main__':
    main()
