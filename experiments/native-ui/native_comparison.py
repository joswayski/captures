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
import statistics
import subprocess
import sys
import tempfile
import time

from benchmark import process_tree, resources, stop


def visible(title):
    result=subprocess.run(['xdotool','search','--onlyvisible','--name',title],capture_output=True,text=True)
    return result.stdout.strip().splitlines()[-1] if result.returncode==0 else None


def trial(binary, implementation, state, lab, artifacts, settle, idle):
    with tempfile.TemporaryDirectory(prefix='captures-comparison-') as directory:
        profile=Path(directory)
        config=profile/'config/captures'
        config.mkdir(parents=True)
        (config/'settings.json').write_text(json.dumps(dict(
            settings_schema_version=5,onboarding_completed=True,
            output_directory=str(profile/'captures'),launch_at_login=False,
            region_shortcut='Super+Shift+S',window_shortcut='Alt+PrintScreen',display_shortcut='Shift+PrintScreen',
        )))
        env=dict(os.environ,HOME=str(profile),XDG_CONFIG_HOME=str(profile/'config'),
                 XDG_DATA_HOME=str(profile/'data'),XDG_CACHE_HOME=str(profile/'cache'),
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
                    window=visible(title)
                    if process.poll() is not None or time.perf_counter()-start>30:
                        log.seek(0)
                        raise RuntimeError(f'{implementation} {state} failed: {log.read().decode(errors="replace")}')
                    time.sleep(.005)
                mapped_ms=(time.perf_counter()-start)*1000
                time.sleep(settle)
                memory=resources(process.pid)
                before=process_tree(process.pid)
                started=time.perf_counter()
                time.sleep(idle)
                elapsed=time.perf_counter()-started
                after=process_tree(process.pid)
                if before.keys()!=after.keys() or process.poll() is not None:
                    raise RuntimeError('Process tree changed during idle sample')
                if artifacts:
                    subprocess.run(['import','-window',window,str(artifacts/f'{implementation}-{state}-measured.png')],check=True)
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
    args=parser.parse_args()
    os.environ.update(json.loads((args.lab/'environment.json').read_text()))
    args.artifacts.mkdir(parents=True,exist_ok=True)
    root=Path(__file__).parent.resolve()
    binaries={'native':root/'target/release/captures-linux-native','tauri':root/'target/release/captures'}
    # Video playback is excluded: this orb's actual Tauri player reports
    # NotSupportedError. A black/error player is not a valid matched workload.
    states=['preferences','image-editor','video-editor'] if args.inspect else ['preferences','image-editor']
    samples={state:{name:[] for name in binaries} for state in states}
    for state,by_name in samples.items():
        for name,binary in binaries.items():
            print(f'Visual warmup: {name} {state}',file=sys.stderr,flush=True)
            trial(binary,name,state,args.lab,args.artifacts,args.settle,args.idle)
        if args.inspect:
            continue
        for index in range(args.runs):
            for name in (['native','tauri'] if index%2==0 else ['tauri','native']):
                result=trial(binaries[name],name,state,args.lab,None,args.settle,args.idle)
                by_name[name].append(result)
                print(f'{state} {index+1}/{args.runs} {name}: {result}',file=sys.stderr,flush=True)
    if args.inspect:
        return
    print(json.dumps(dict(recorded_at=datetime.now(timezone.utc).isoformat(),
        environment='Debian 12, Xvfb 1280x800, Openbox+xcompmgr, software GL, 2 vCPU; isolated verified-unlocked DBus fixture',
        scope='Whole source Tauri app versus partial native frontend; shared input files, unequal feature completeness. No performance extrapolation to macOS/Windows.',
        timing='Time to first mapped native target window, not rendered content readiness; 5s settling then 2s idle CPU sample.',
        binaries={name:dict(bytes=p.stat().st_size,sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for name,p in binaries.items()},
        inputs={name:hashlib.sha256((args.lab/name).read_bytes()).hexdigest() for name in ['fixture.png','fixture.mp4']},
        excluded='Video playback: Tauri reports NotSupportedError in this orb. Native uses frame scrubbing and external playback; no matched playback benchmark.',
        samples=samples,
        medians={state:{name:{key:statistics.median(row[key] for row in rows) for key in rows[0]} for name,rows in by_name.items()} for state,by_name in samples.items()},
    ),indent=2))


if __name__=='__main__':
    main()
