#!/usr/bin/python3
"""Whole-source-app vs native feature-state memory; not a feature-parity claim.

Both open the same input files. The Tauri process includes its startup webviews.
First mapped window timing is NOT UI readiness or first presented content.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import sys
import tempfile
import time

from benchmark import process_tree, resources, stop


WINDOW_SIZES = {'preferences': (980, 720), 'image-editor': (1280, 760), 'video-editor': (1280, 760)}


def visible(title, pid):
    result=subprocess.run(['xdotool','search','--onlyvisible','--all','--pid',str(pid),'--name',title],capture_output=True,text=True)
    return result.stdout.strip().splitlines()[-1] if result.returncode==0 else None


def capture_window(window, screenshot, expected):
    # xdotool can double-count a reparented client's frame offset. xwininfo's
    # absolute coordinates identify the actual compositor pixels to capture.
    info=subprocess.check_output(['xwininfo','-id',window],text=True,
                                 env=dict(os.environ,LC_ALL='C'))
    geometry={key:int(re.search(rf'^\s*{label}:\s*(-?\d+)',info,re.MULTILINE).group(1))
              for key,label in [('X','Absolute upper-left X'),('Y','Absolute upper-left Y'),
                                ('WIDTH','Width'),('HEIGHT','Height')]}
    actual=tuple(geometry[key] for key in ('WIDTH','HEIGHT'))
    if actual != expected:
        raise RuntimeError(f'Window viewport is {actual}, expected {expected}')
    # GPU surfaces may not appear in the client's backing pixmap. Read the
    # composited screen at the actual client bounds instead.
    crop=f'{actual[0]}x{actual[1]}{int(geometry["X"]):+d}{int(geometry["Y"]):+d}'
    subprocess.run(['import','-window','root','-crop',crop,'+repage',str(screenshot)],check=True)
    header=screenshot.read_bytes()[:24]
    if header[:8] != b'\x89PNG\r\n\x1a\n' or header[12:16] != b'IHDR':
        raise RuntimeError('The compositor capture is not a PNG')
    dimensions=tuple(int.from_bytes(header[offset:offset+4], 'big') for offset in (16, 20))
    if dimensions != expected:
        raise RuntimeError(f'Compositor capture is {dimensions}, expected {expected}; client geometry: {geometry}')
    if int(subprocess.check_output(['identify','-format','%k',str(screenshot)],text=True)) <= 1:
        raise RuntimeError('The captured window is a blank, single-color surface')


def trial(binary, implementation, state, lab, artifacts, settle, idle, appearance='dark'):
    with tempfile.TemporaryDirectory(prefix='captures-comparison-') as directory:
        profile=Path(directory)
        config=profile/'config/captures'
        config.mkdir(parents=True)
        (config/'settings.json').write_text(json.dumps(dict(
            settings_schema_version=5,onboarding_completed=True,
            appearance=appearance,
            show_mini_previews=False,
            output_directory=str(profile/'captures'),launch_at_login=False,
            region_shortcut='Super+Shift+S',window_shortcut='Alt+PrintScreen',display_shortcut='Shift+PrintScreen',
        )))
        native=profile/'native'
        native.mkdir()
        (native/'settings.json').write_text(json.dumps(dict(
            appearance=appearance,show_mini_previews=False,
            output_directory=str(profile/'captures'),launch_at_login=False,
        )))
        runtime=profile/'runtime'
        runtime.mkdir(mode=0o700)
        env=dict(os.environ,HOME=str(profile),XDG_CONFIG_HOME=str(profile/'config'),
                 XDG_DATA_HOME=str(profile/'data'),XDG_CACHE_HOME=str(profile/'cache'),
                 XDG_RUNTIME_DIR=str(runtime),
                 CAPTURES_NATIVE_DATA=str(profile/'native'),LIBGL_ALWAYS_SOFTWARE='1',
                 HTTP_PROXY='http://127.0.0.1:9',HTTPS_PROXY='http://127.0.0.1:9',ALL_PROXY='http://127.0.0.1:9',NO_PROXY='localhost,127.0.0.1')
        if state=='preferences':
            args=['--preferences'] if implementation=='native' else []
            title='Preferences'
        else:
            fixture=lab/('fixture.png' if state=='image-editor' else 'fixture.mp4')
            args=['--open',str(fixture)] if implementation=='native' else [str(fixture)]
            title=('Image editor' if implementation=='native' else 'Screenshot') if state=='image-editor' else ('Edit recording' if implementation=='native' else '^Captures Editor$')
        with tempfile.TemporaryFile() as log:
            start=time.perf_counter()
            process=subprocess.Popen([str(binary),*args],env=env,stdout=log,stderr=log,start_new_session=True)
            try:
                window=None
                while window is None:
                    window=visible(title,process.pid)
                    if process.poll() is not None or time.perf_counter()-start>30:
                        log.seek(0)
                        raise RuntimeError(f'{implementation} {state} failed: {log.read().decode(errors="replace")}')
                    time.sleep(.005)
                mapped_ms=(time.perf_counter()-start)*1000
                # Match client viewport area before settling/memory samples.
                # Mapping timing ends above, before this common resize.
                subprocess.run(['xdotool', 'windowsize', '--sync', window,
                                *map(str, WINDOW_SIZES[state])], check=True)
                # Keep the WM in control: changing override_redirect on an
                # already-managed window races Openbox's subsequent placement.
                # The lab must fit the client plus its window decorations.
                subprocess.run(['xdotool', 'windowmove', '--sync', window, '0', '0'], check=True)
                time.sleep(settle)
                memory=resources(process.pid)
                before=process_tree(process.pid)
                if len(before) != memory['processes']:
                    raise RuntimeError('Process tree changed while collecting memory')
                started=time.perf_counter()
                time.sleep(idle)
                elapsed=time.perf_counter()-started
                after=process_tree(process.pid)
                if before.keys()!=after.keys() or process.poll() is not None:
                    raise RuntimeError('Process tree changed during idle sample')
                if not visible(title,process.pid):
                    raise RuntimeError('The measured window disappeared')
                # Run after CPU sampling, so screenshot work does not inflate
                # the app's idle measurement. Reject blank output in every run.
                screenshot = ((artifacts/f'{implementation}-{state}-measured.png')
                              if artifacts else profile/'measured.png')
                capture_window(window,screenshot,WINDOW_SIZES[state])
                return dict(memory,first_window_mapped_ms=mapped_ms,
                            idle_cpu_percent_one_core=100*(sum(after.values())-sum(before.values()))/os.sysconf('SC_CLK_TCK')/elapsed)
            finally:
                stop(process)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab',type=Path,required=True)
    parser.add_argument('--artifacts',type=Path,required=True)
    parser.add_argument('--runs',type=int,default=5)
    parser.add_argument('--settle',type=float,default=5)
    parser.add_argument('--idle',type=float,default=2)
    parser.add_argument('--inspect',action='store_true',help='One visual trial per state; not benchmark results')
    parser.add_argument('--native',type=Path,help='Full native release app, not the minimal GTK probe')
    parser.add_argument('--tauri',type=Path,help='Current source Tauri release app')
    parser.add_argument('--native-label',default='GTK3/Cairo',help='Record the actual native frontend being measured')
    parser.add_argument('--states',nargs='+',choices=list(WINDOW_SIZES),default=['preferences','image-editor'])
    parser.add_argument('--appearance',choices=['light','dark'],default='dark')
    args=parser.parse_args()
    if args.runs < 2 or args.settle < 0 or args.idle <= 0:
        parser.error('Use at least two trials, nonnegative settling time and positive idle duration')
    os.environ.update(json.loads((args.lab/'environment.json').read_text()))
    args.artifacts.mkdir(parents=True,exist_ok=True)
    root=Path(__file__).parent.resolve()
    binaries={'native':(args.native or root/'target/release/captures-linux-native').resolve(),
              'tauri':(args.tauri or root/'target/release/captures').resolve()}
    for binary in binaries.values():
        if not binary.is_file() or not os.access(binary,os.X_OK):
            parser.error(f'Not an executable file: {binary}')
    # Video playback is excluded: this orb's actual Tauri player reports
    # NotSupportedError. A black/error player is not a valid matched workload.
    # Opt in explicitly only after playback works in both implementations.
    states=args.states
    samples={state:{name:[] for name in binaries} for state in states}
    for state,by_name in samples.items():
        for name,binary in binaries.items():
            print(f'Visual warmup: {name} {state}',file=sys.stderr,flush=True)
            trial(binary,name,state,args.lab,args.artifacts,args.settle,args.idle,args.appearance)
        if args.inspect:
            continue
        for index in range(args.runs):
            for name in (['native','tauri'] if index%2==0 else ['tauri','native']):
                result=trial(binaries[name],name,state,args.lab,None,args.settle,args.idle,args.appearance)
                by_name[name].append(result)
                print(f'{state} {index+1}/{args.runs} {name}: {result}',file=sys.stderr,flush=True)
    if args.inspect:
        return
    print(json.dumps(dict(recorded_at=datetime.now(timezone.utc).isoformat(),
        environment=dict(system=os.uname().sysname,release=os.uname().release,cpu_count=os.cpu_count(),
                         display_geometry=subprocess.check_output(['xdotool','getdisplaygeometry'],text=True).strip(),
                         lab='Isolated X11/Openbox/xcompmgr/DBus fixture; software rendering; not physical hardware'),
        scope=f'Whole source Tauri app versus {args.native_label}; shared input files, remaining OS/interaction differences. No performance extrapolation to macOS/Windows.',
        appearance=args.appearance,
        show_mini_previews=False,
        timing=f'Time to first mapped target window, not rendered content readiness; {args.settle:g}s settling then {args.idle:g}s idle CPU sample.',
        client_viewports=WINDOW_SIZES,
        source_base=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
        binaries={name:dict(bytes=p.stat().st_size,sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for name,p in binaries.items()},
        inputs={name:hashlib.sha256((args.lab/name).read_bytes()).hexdigest() for name in
                (['fixture.png','fixture.mp4'] if 'video-editor' in states else ['fixture.png'])},
        excluded='Only listed states are measured. Capture/encoding throughput, GPU allocation, energy, animation FPS and content-ready latency are not measured. Video requires separately verified playback in both apps.',
        samples=samples,
        medians={state:{name:{key:statistics.median(row[key] for row in rows) for key in rows[0]} for name,rows in by_name.items()} for state,by_name in samples.items()},
    ),indent=2))


if __name__=='__main__':
    main()
