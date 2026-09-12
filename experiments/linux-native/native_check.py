#!/usr/bin/python3
"""Real GTK/X11 interaction checks. Requires native_desktop.py's disposable session.

No test-only application IPC: use AT-SPI controls and X11 pointer/keyboard events.
"""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from process_metrics import stop


def cmd(*args):
    return subprocess.check_output([str(a) for a in args], text=True).strip()


def walk(node):
    yield node
    try:
        for child in node:
            yield from walk(child)
    except Exception:
        pass


def find(name=None, role=None, frame=None):
    import pyatspi
    hidden_matches = []
    for app in pyatspi.Registry.getDesktop(0):
        if app.name != 'captures-linux-native':
            continue
        roots = [app] if frame is None else [n for n in app if n.name == frame]
        for root in roots:
            for node in walk(root):
                try:
                    if ((name is None or node.name == name) and
                            (role is None or node.getRoleName() == role)):
                        if node.getState().contains(pyatspi.STATE_SHOWING):
                            return node
                        hidden_matches.append(node)
                except Exception:
                    pass
    # GTK4/AT-SPI can temporarily clear SHOWING for an otherwise mapped X11
    # subtree after a compositor screenshot. A unique match is still safe to
    # operate; hidden popover controls with duplicate names remain ambiguous.
    if len(hidden_matches) == 1:
        return hidden_matches[0]
    controls = [node for node in hidden_matches
                if node.getRoleName() not in ('label', 'filler', 'panel')]
    if len(controls) == 1:
        return controls[0]
    actionable = []
    for node in hidden_matches:
        try:
            if node.queryAction().nActions:
                actionable.append(node)
        except Exception:
            pass
    return actionable[0] if len(actionable) == 1 else None


def wait(predicate, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError('Timed out waiting for ' + repr(predicate))


def click(name, frame=None, pointer=False):
    node = wait(lambda: find(name, frame=frame))
    if pointer:
        # Opening a modal dialog from AT-SPI's synchronous DoAction keeps its
        # DBus handler occupied until that dialog closes. Use real X11 input.
        import pyatspi
        bounds=node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
        cmd('xdotool','mousemove',bounds.x+bounds.width//2,bounds.y+bounds.height//2,'click',1)
    else:
        assert node.queryAction().doAction(0), name
    time.sleep(.15)


def choose(current, index, frame=None):
    import pyatspi
    node = wait(lambda: find(current, 'combo box', frame))
    title=frame or 'Captures — Linux native'
    window=cmd('xdotool','search','--onlyvisible','--name',title).splitlines()[-1]
    cmd('xdotool','windowactivate','--sync',window)
    bounds = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    cmd('xdotool', 'mousemove', bounds.x + bounds.width // 2,
        bounds.y + bounds.height // 2, 'click', 1)
    time.sleep(.1)
    cmd('xdotool', 'key', 'Home', *(['Down'] * index), 'Return')
    time.sleep(.15)


def drag(x, y, dx, dy):
    cmd('xdotool', 'mousemove', x, y, 'mousedown', 1)
    for i in range(1, 13):
        cmd('xdotool', 'mousemove', x+dx*i//12, y+dy*i//12)
        time.sleep(.015)
    cmd('xdotool', 'mouseup', 1)
    time.sleep(.2)


def capture(artifacts, name, title=None):
    def visible_window():
        result=subprocess.run(['xdotool','search','--onlyvisible','--name',title],capture_output=True,text=True)
        return result.stdout.strip().splitlines()[-1] if result.returncode==0 else None
    window = 'root' if title is None else wait(visible_window)
    time.sleep(.3)
    cmd('import','-window',window,artifacts / f'after-{name}.png')


def run(binary, fixture, args, artifacts):
    import pyatspi
    with tempfile.TemporaryDirectory(prefix='captures-native-check-') as temporary:
        profile = Path(temporary)
        output = profile / 'captures'
        env = dict(os.environ, CAPTURES_NATIVE_DATA=str(profile))
        with (profile/'log').open('w+') as log:
            process = subprocess.Popen([str(binary), *args],env=env,stdout=log,stderr=log,start_new_session=True)
            try:
                wait(lambda: find(role='frame'))
                time.sleep(.5)
                yield process, output
            except Exception:
                log.seek(0)
                print(log.read(),flush=True)
                print(cmd('xdotool','search','--onlyvisible','--name','Captures'),flush=True)
                cmd('import','-window','root','/tmp/captures-native-failure.png')
                for app in pyatspi.Registry.getDesktop(0):
                    if app.name=='captures-linux-native':
                        for node in walk(app):
                            if node.name: print(node.getRoleName(),node.name,flush=True)
                raise
            finally:
                stop(process)
                time.sleep(.3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()

    root = Path(__file__).resolve().parent
    binary = (root / 'target/release/captures-linux-native').resolve()
    checks = [
        (root / 'native_parity_check.py', '--lab', args.lab, '--artifacts', args.artifacts),
        (root / 'editor_check.py', '--lab', args.lab, '--artifacts', args.artifacts),
        (root / 'preview_check.py', '--lab', args.lab, '--artifacts', args.artifacts,
         '--binary', binary),
        (root / 'history_check.py', '--lab', args.lab),
        (root / 'src/native/recording/check.py', '--lab', args.lab,
         '--artifacts', args.artifacts),
    ]
    for check in checks:
        subprocess.run([sys.executable, *(str(value) for value in check)], check=True)


if __name__ == '__main__':
    main()
