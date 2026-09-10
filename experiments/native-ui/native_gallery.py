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
    ('capture-menu', 'Capture menu', 'Two-step native form, not menu parity.'),
    ('region', 'Region screenshot selection', 'Reverse-drag selection; native primary display only.'),
    ('window', 'Window screenshot selection', 'Native X11 window targeting; multi-monitor parity is unverified.'),
    ('display', 'Full-display screenshot selection', 'Native confirms the primary display; Tauri also has a display picker.'),
    ('previews-collapsed', 'Collapsed mini previews', 'Both stack images; native layout and actions differ.'),
    ('previews-expanded', 'Expanded mini previews', 'Native Cairo stack; full per-card actions are incomplete.'),
    ('image-editor', 'Image editor - identical input', 'Actual Tauri app versus simpler native layer editor.'),
    ('recording', 'Recording controls', 'Real native pause/resume/stop; restart and live mic controls are missing.'),
    ('video-editor', 'Video editor', 'Native frame scrubbing and numeric edits; playback opens a system player.'),
    ('history', 'Capture history', 'Native separate directory list, not the full history browser.'),
    ('preferences', 'Preferences', 'Actual Tauri app versus the native subset of settings.'),
]


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
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
                if name == 'display' and before:
                    filename = 'before-capture-menu.png'  # Already in full-display mode.
                label = ('TAURI / BEFORE - ' + ('actual app' if measured else 'React visual harness')) if before else 'GTK NATIVE / AFTER - actual app'
                output = work / f'{before}.miff'
                crop = []
                if name.startswith('previews-'):
                    # Normalize to CSS/display coordinates, then crop the stack
                    # area, not just the cards. Source images remain untouched.
                    crop = ['-resize', '1280x800!', '-crop', '540x800+0+0', '+repage']
                run('convert', args.artifacts / filename, *crop, '-thumbnail', '760x475',
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
    sections.append('<section><h2>Native transparent canvas</h2><p>Basic canvas creation uses the native image editor; not a separate parity claim.</p><img src="canvas.webp" alt="Native blank transparent canvas"></section>')
    for title, filename in [('Tauri dust - React harness action', 'tauri-dust.webm'), ('Native dust - GTK/Cairo action', 'native-dust.mp4')]:
        source = args.artifacts / filename
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
<p class="notice"><strong>Working Linux X11 implementation, not full parity.</strong> Native preserves the possibility of stacking and particles, but this app has simpler editors and incomplete OS integration. Tauri remains the shipping app. These screenshots are not benchmark evidence.</p>
<p>Before: actual Tauri Preferences/image editor; other before views use the React dev harness. After: actual GTK/Cairo windows. Images are scaled to fit; preview pairs crop the stack area (background windows can be cut off). Different sample content is intentional except the identical image-editor benchmark input. Click an image for its full-size comparison.</p>
<p><a href="https://github.com/joswayski/captures/pull/512">Pull request, feature matrix, measured results and reproduction</a></p>
'''
    (args.output/'index.html').write_text(page + '\n'.join(sections) + '</main></html>')


if __name__ == '__main__':
    main()
