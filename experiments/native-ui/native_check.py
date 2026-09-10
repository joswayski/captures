#!/usr/bin/python3
"""Real GTK/X11 interaction checks. Requires native_desktop.py's disposable session.

No test-only application IPC: use AT-SPI controls and X11 pointer/keyboard events.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from benchmark import stop


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
    for app in pyatspi.Registry.getDesktop(0):
        if app.name != 'captures-linux-native':
            continue
        roots = [app] if frame is None else [n for n in app if n.name == frame]
        for root in roots:
            for node in walk(root):
                try:
                    if ((name is None or node.name == name) and
                            (role is None or node.getRoleName() == role) and
                            node.getState().contains(pyatspi.STATE_SHOWING)):
                        return node
                except Exception:
                    pass
    return None


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
    node = wait(lambda: find(current, 'combo box', frame))
    title=frame or 'Captures — Linux native'
    window=cmd('xdotool','search','--onlyvisible','--name',title).splitlines()[-1]
    cmd('xdotool','windowactivate','--sync',window)
    node.queryAction().doAction(0)
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
                wait(lambda: find('Captures — Linux native','frame'))
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
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab',type=Path,required=True)
    parser.add_argument('--artifacts',type=Path,required=True)
    args=parser.parse_args()
    os.environ.update(json.loads((args.lab/'environment.json').read_text()))
    # Import AT-SPI only after applying this isolated session environment.
    import pyatspi
    args.artifacts.mkdir(parents=True,exist_ok=True)
    binary=Path(__file__).parent.resolve()/'target/release/captures-linux-native'
    fixture=args.lab/'fixture.png'
    from contextlib import contextmanager
    launch=contextmanager(run)
    editor='Captures — Image editor'
    import dbus
    saver=dbus.Interface(dbus.SessionBus().get_object('org.freedesktop.ScreenSaver',
                         '/org/freedesktop/ScreenSaver'),'org.freedesktop.ScreenSaver')
    with launch(binary,fixture,[],args.artifacts) as (process,output):
        click('New canvas…',pointer=True)
        click('Transparent', 'New canvas')
        click('Create', 'New canvas')
        wait(lambda:find(editor,'frame'))
        click('Export',editor)
        blank=wait(lambda:next(output.glob('*.png'),None))
        assert cmd('identify','-format','%wx%h %[opaque]',blank)=='960x540 false'
        capture(args.artifacts,'canvas','Image editor')
        print('PASS canvas: transparent 960x540 PNG exported from native creation dialog',flush=True)

    with launch(binary,fixture,[],args.artifacts) as (process,output):
        click('Choose target')
        wait(lambda:find('Confirm'))
        drag(710,460,-380,-220)
        try:
            saver.SetActive(True)
            click('Confirm')
            click('Close')
            wait(lambda:find('Choose target'))
            assert not list(output.glob('*.png')),'Capture published while session locked'
        finally:
            saver.SetActive(False)
        click('Choose target')
        wait(lambda:find('Confirm'))
        drag(710,460,-380,-220)
        click('Confirm')
        wait(lambda:next(output.glob('*.png'),None))
        print('PASS session: lock during selection cancels capture, restores menu, permits retry',flush=True)

    with launch(binary,fixture,['--open',str(fixture)],args.artifacts) as (process,output):
        wait(lambda:find(editor,'frame'))
        click('Arrow',editor)
        area=find(role='drawing area',frame=editor).queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
        drag(area.x+100,area.y+110,250,160)
        capture(args.artifacts,'image-editor','Image editor')
        click('Export',editor)
        first=wait(lambda:next(output.glob('*.png'),None))
        assert cmd('identify','-format','%wx%h',first)=='960x540'
        assert cmd('convert',first,'-format','%[pixel:p{225,190}]','info:') != cmd('convert',fixture,'-format','%[pixel:p{225,190}]','info:')
        click('Undo',editor)
        click('Export',editor)
        wait(lambda:len(list(output.glob('*.png')))==2)
        second=sorted(output.glob('*.png'))[-1]
        assert cmd('compare','-metric','AE',fixture,second,'null:') == ''
        click('Redo',editor)
        click('Crop',editor)
        drag(area.x+470,area.y+320,-360,-180)
        click('Export',editor)
        wait(lambda:len(list(output.glob('*.png')))==3)
        cropped=sorted(output.glob('*.png'))[-1]
        assert cmd('identify','-format','%wx%h',cropped)=='360x180'
        choose('PNG',1,editor)
        click('Export',editor)
        jpeg=wait(lambda:next(output.glob('*.jpg'),None))
        assert cmd('identify','-format','%m %wx%h',jpeg)=='JPEG 360x180'
        print('PASS image: arrow pixels, undo exact source, redo, reverse crop, PNG/JPEG export',flush=True)

    with launch(binary,fixture,[],args.artifacts) as (process,output):
        capture(args.artifacts,'capture-menu','Linux native')
        click('Choose target')
        wait(lambda:find('Captures — Select target','frame'))
        drag(710,460,-380,-220)
        capture(args.artifacts,'region')
        click('Confirm')
        first=wait(lambda:next(output.glob('*.png'),None))
        assert cmd('identify','-format','%wx%h',first)=='380x220'
        choose('Region',2)
        click('Choose target')
        wait(lambda:find('Confirm'))
        capture(args.artifacts,'display')
        click('Confirm')
        wait(lambda:len(list(output.glob('*.png')))==2)
        assert cmd('identify','-format','%wx%h',sorted(output.glob('*.png'))[-1])=='1280x800'
        choose('Full display',1)
        click('Choose target')
        wait(lambda:find('Confirm'))
        cmd('xdotool','mousemove',650,300)
        time.sleep(.2)
        capture(args.artifacts,'window')
        click('Confirm')
        wait(lambda:len(list(output.glob('*.png')))==3)
        third=sorted(output.glob('*.png'))[-1]
        dimensions=cmd('identify','-format','%wx%h',third)
        assert dimensions != '1280x800' and dimensions != '380x220', dimensions
        capture(args.artifacts,'previews-collapsed')
        click('Expand')
        time.sleep(.6)
        capture(args.artifacts,'previews-expanded')
        clip=subprocess.Popen(['ffmpeg','-v','error','-y','-f','x11grab','-video_size','1280x800','-framerate','30','-i',os.environ['DISPLAY'],'-t','5','-c:v','libx264','-preset','ultrafast','-tune','zerolatency','-pix_fmt','yuv420p','-progress','pipe:1','-stats_period','0.1',str(args.artifacts/'native-dust.mp4')],stdout=subprocess.PIPE,text=True)
        # Wait for actual encoded frames, not a guessed FFmpeg startup delay.
        for line in clip.stdout:
            if line.startswith('frame=') and int(line.split('=')[1])>0:
                break
        else:
            raise AssertionError('Desktop recording did not produce a frame')
        time.sleep(.3)
        click('Dismiss')
        time.sleep(.4)
        capture(args.artifacts,'dust')
        clip.wait()
        assert clip.returncode==0,'Desktop animation recording failed'
        assert len(list(output.glob('*.png')))==3,'Dismiss removed saved history'
        click('History')
        capture(args.artifacts,'history','History')
        click('Preferences')
        capture(args.artifacts,'preferences','Preferences')
        print('PASS capture: reverse region, display, window, stack expand/dust; dismissal retains files',flush=True)

    with launch(binary,fixture,[],args.artifacts) as (process,output):
        choose('Screenshot',1)
        choose('Region',2)
        click('Choose target')
        wait(lambda:find('Confirm'))
        click('Confirm')
        wait(lambda:find('Pause'))
        time.sleep(2)
        capture(args.artifacts,'recording','recording controls')
        click('Pause')
        wait(lambda:find('Resume'))
        time.sleep(.4)
        click('Resume')
        wait(lambda:find('Pause'))
        time.sleep(1.5)
        click('Stop')
        video=wait(lambda:next((p for p in output.glob('*.mp4') if p.stat().st_size>1000),None),45)
        wait(lambda:find('Edit recording — Captures','frame'))
        time.sleep(1)
        capture(args.artifacts,'video-editor','Edit recording')
        metadata=json.loads(cmd('ffprobe','-v','error','-show_streams','-of','json',video))['streams'][0]
        assert (metadata['width'],metadata['height'])==(1280,800)
        assert float(metadata['duration'])>=2.5
        print('PASS recording: real xcap full display, pause/resume, final MP4 verified by ffprobe',flush=True)

    for kind,target,extension in [(2,0,'gif'),(1,1,'mp4')]:
        with launch(binary,fixture,[],args.artifacts) as (process,output):
            choose('Screenshot',kind)
            if target:
                choose('Region',target)
            click('Choose target')
            wait(lambda:find('Confirm'))
            if target:
                cmd('xdotool','mousemove',10,10,'mousemove',650,300)
                wait(lambda:find('Confirm').getState().contains(pyatspi.STATE_SENSITIVE))
            else:
                drag(710,460,-380,-220)
            click('Confirm')
            wait(lambda:find('Pause'))
            time.sleep(1.5)
            if target:
                try:
                    saver.SetActive(True)
                    wait(lambda:find('Resume'))
                finally:
                    saver.SetActive(False)
                click('Resume')
                wait(lambda:find('Pause'))
                time.sleep(.8)
            click('Stop')
            video=wait(lambda:next((p for p in output.glob('*.'+extension) if p.stat().st_size>1000),None),45)
            wait(lambda:find('Edit recording — Captures','frame'))
            metadata=json.loads(cmd('ffprobe','-v','error','-show_streams','-of','json',video))['streams'][0]
            assert float(metadata['duration'])>=.8
            if target:
                assert (metadata['width'],metadata['height']) not in [(1280,800),(380,220)]
                print('PASS recording: window target, session-lock automatic pause and manual resume',flush=True)
            else:
                assert (metadata['width'],metadata['height'])==(380,220)
                assert metadata['codec_name']=='gif'
                capture(args.artifacts,'gif-editor','Edit recording')
                print('PASS recording: reverse-region GIF, actual GIF codec and 380x220 output',flush=True)


if __name__=='__main__':
    main()
