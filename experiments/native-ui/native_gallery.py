#!/usr/bin/python3
"""Compose labelled review images from inspected screenshots; requires ImageMagick.

Inputs are native_check.py/native_comparison.py captures plus the documented React
dev harness's before-*.png captures. This renders a report, never app screenshots.
"""
import argparse
import html
from pathlib import Path
import subprocess
import tempfile


PAIRS = [
    ('capture-menu', 'Capture menu', 'Original segmented toolbar and guidance; no additional native launcher.'),
    ('recording-selector', 'Recording selection', 'The same command bar reveals recording options when Record is selected.'),
    ('region', 'Region screenshot selection', 'Native move/resize, aspect ratios, frozen/live selection and countdown.'),
    ('window', 'Window screenshot selection', 'Native X11 window targeting; multi-monitor parity is unverified.'),
    ('display', 'Full-display screenshot selection', 'Native display picker; physical multi-monitor/HiDPI testing remains open.'),
    ('previews-collapsed', 'Collapsed mini previews', 'Four-corner receding stack, fan, drag placement and click-through.'),
    ('previews-expanded-idle', 'Expanded mini previews - idle', 'Image cards and metadata; action chrome stays hidden until hover or focus.'),
    ('previews-expanded', 'Expanded mini previews - hover', 'Source-shaped icon actions, image blur, paging and confirmed dust delete.'),
    ('image-editor', 'Image editor - identical input', 'Layered native editor with transforms, background removal and persistent drafts.'),
    ('recording', 'Recording controls', 'Real pause/resume, restart, live microphone changes and screenshot callback.'),
    ('video-editor', 'Video editor', 'In-window playback, trim/crop, quality comparison and MP4/GIF exports.'),
    ('history', 'Capture history', 'Native thumbnails, filtering, preview restore and non-destructive history removal.'),
    ('preferences', 'Preferences', 'One scrolling settings document with section navigation and automatic saving.'),
]


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--before-artifacts', type=Path, help='Existing Tauri harness captures; defaults to --artifacts')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.before_artifacts = args.before_artifacts or args.artifacts
    args.output.mkdir(parents=True, exist_ok=True)
    sections = []
    with tempfile.TemporaryDirectory(prefix='captures-gallery-') as work:
        work = Path(work)
        for name, title, note in PAIRS:
            measured = name in ['preferences', 'image-editor']
            cards = []
            for before in [True, False]:
                if measured:
                    filename = f'{"tauri" if before else "native"}-{name}-measured.png'
                else:
                    filename = f'{"before" if before else "after"}-{name}.png'
                if not before:
                    filename = {
                        'previews-collapsed': 'after-preview-bottom-left-collapsed.png',
                        'previews-expanded-idle': 'after-preview-expanded-idle.png',
                        'previews-expanded': 'after-preview-expanded-hover.png',
                        'recording': 'recording-hud.png',
                        'video-editor': 'recording-editor.png',
                    }.get(name, filename)
                source_directory = args.before_artifacts if before and not measured else args.artifacts
                label = ('TAURI / BEFORE - ' + ('actual app' if measured else 'React visual harness')) if before else 'GTK NATIVE / AFTER - actual app'
                output = work / f'{before}.miff'
                crop = []
                if name.startswith('previews-'):
                    # React uses a 340x760 CSS viewport at DPR 2. GTK's same
                    # bottom-left window sits at (0,40) in the 1280x800 lab.
                    crop = (['-resize', '340x760!'] if before else
                            ['-crop', '340x760+0+40', '+repage'])
                run('convert', source_directory / filename, *crop, '-thumbnail', '760x475',
                    '-background', '#ededed', '-gravity', 'center', '-extent', '780x495',
                    '-gravity', 'north', '-splice', '0x48', '-font', 'DejaVu-Sans',
                    '-pointsize', '19', '-fill', '#202124', '-annotate', '+0+14', label, output)
                cards.append(output)
            run('montage', *cards, '-tile', '2x1', '-geometry', '+8+8', '-background', '#fafafa', work/'pair.miff')
            run('convert', work/'pair.miff', '-gravity', 'north', '-splice', '0x90',
                '-font', 'DejaVu-Sans', '-pointsize', '28', '-fill', '#202124', '-annotate', '+0+14', title,
                '-pointsize', '18', '-annotate', '+0+55', note, '-quality', '88', args.output/f'{name}.webp')
            sections.append(f'<section><h2>{html.escape(title)}</h2><p>{html.escape(note)}</p>'
                            f'<a href="{name}.webp"><img src="{name}.webp" alt="{html.escape(title)} before and after"></a></section>')
        run('convert', args.artifacts/'after-canvas.png', '-quality', '88', args.output/'canvas.webp')
    sections.append('<section><h2>Native transparent canvas</h2><p>Canvas authoring uses the same layer editor. Checkerboard is UI-only; exported pixels retain alpha.</p><img src="canvas.webp" alt="Native blank transparent canvas"></section>')
    for title, filename in [('Tauri dust - React harness action', 'tauri-dust.webm'), ('Native dust - GTK/Cairo action', 'native-dust.mp4')]:
        source = (args.before_artifacts / filename) if filename.startswith('tauri') else (args.artifacts / 'after-preview-delete-dust.mp4')
        if source.exists():
            import shutil
            if source.resolve() != (args.output/filename).resolve():
                shutil.copyfile(source, args.output/filename)
            sections.append(f'<section><h2>{title}</h2><video src="{filename}" controls muted playsinline></video></section>')
    page = '''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Captures: Linux native review</title><style>
body{margin:0;background:#fafafa;color:#202124;font:16px/1.6 system-ui,sans-serif}main{max-width:1500px;margin:auto;padding:32px}
h1{font-size:32px;line-height:1.2}h2{font-size:22px}p{max-width:950px}section{margin:48px 0;border-top:1px solid #ddd;padding-top:16px}
img,video{display:block;max-width:100%;height:auto;margin:12px 0}video{width:960px}.notice{border-left:4px solid #444;padding-left:16px}
</style><main><h1>Captures: Linux native before / after</h1>
<p class="notice"><strong>Linux-native fidelity pass, not a certified no-regressions replacement.</strong> These layouts now follow the existing React surfaces rather than an alternative GTK design. Stacking, particles, layered editing, embedded video playback and recovery remain implemented. Wayland, distribution and physical desktop validation are still open. Tauri remains the shipping app. Screenshots are not benchmark evidence.</p>
<p>Before: actual Tauri Preferences/image editor; other before views use the React dev harness. After: actual GTK/Cairo windows. Images are scaled to fit; preview pairs show equivalent 340x760 stack frames. The image-editor pair uses identical neutral input; other test content can differ. Preview test colors distinguish file identity. Native screenshot controls are hidden before capture, so their warning differs from the Linux React harness. Click an image for its full-size comparison.</p>
<p>Native lab: X11/Openbox with normal alpha compositing and no compositor-generated window shadows. GNOME/KDE and Wayland behavior have not been verified.</p>
<p><a href="https://github.com/joswayski/captures/pull/512">Pull request, feature matrix, measured results and reproduction</a></p>
'''
    (args.output/'index.html').write_text(page + '\n'.join(sections) + '</main></html>')


if __name__ == '__main__':
    main()
