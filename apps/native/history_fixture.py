"""Create isolated on-disk screenshot history for native UI checks (no capture access)."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import struct
import subprocess
import uuid
import zlib


def write_completed_settings(path: Path):
    """Existing-profile fixture; first-run behavior has its own UI smoke test."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"settings_schema_version": 5, "onboarding_completed": True,
        "output_directory": str(path.parent / "exports"), "launch_at_login": False,
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10"}))


def fixture_png(width=640, height=360, accent=(220, 169, 65)):
    # Asymmetric pattern exposes stretching, orientation and blank decode failures.
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            rows.extend(accent if x > width * 11 // 16 and y < height * 5 // 18 else
                        (66, 122, 97) if y > height * 29 // 36 else (46, 67, 100))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def write_history(root: Path):
    root.mkdir(parents=True, exist_ok=False)
    width, height = 640, 360
    png = fixture_png(width, height)
    entry_id = str(uuid.uuid4())
    directory = root / entry_id
    directory.mkdir()
    (directory / "capture.png").write_bytes(png)
    (directory / "preview.png").write_bytes(png)
    (directory / "metadata.json").write_text(json.dumps({
        "id": entry_id, "kind": "screenshot", "mode": "display",
        "width": width, "height": height, "size_bytes": len(png),
        "created_at": datetime.now(timezone.utc).isoformat(),
        "preview_url": "", "full_url": "", "mime_type": "image/png",
    }), encoding="utf-8")


def write_history_entries(root: Path, count: int):
    """`count` entries, newest first a minute apart: mostly screenshots, every
    fifth a video and every seventh a GIF (some with dropped frames), in four
    accent colours so recycled cards that show the wrong entry stand out."""
    root.mkdir(parents=True, exist_ok=False)
    accents = [(220, 169, 65), (214, 92, 92), (120, 170, 230), (150, 210, 120)]
    pngs = [fixture_png(accent=accent) for accent in accents]
    now = datetime.now(timezone.utc).timestamp()
    for index in range(count):
        png = pngs[index % len(pngs)]
        entry_id = str(uuid.uuid4())
        directory = root / entry_id
        directory.mkdir()
        (directory / "preview.png").write_bytes(png)
        created = datetime.fromtimestamp(now - index * 60, timezone.utc).isoformat()
        metadata = {"id": entry_id, "width": 640, "height": 360, "size_bytes": len(png),
                    "created_at": created, "preview_url": "", "full_url": ""}
        if index % 5 == 4 or index % 7 == 6:
            gif = index % 5 != 4
            (directory / ("media.gif" if gif else "media.mp4")).write_bytes(b"fixture")
            metadata.update(kind="gif" if gif else "video",
                            mime_type="image/gif" if gif else "video/mp4",
                            duration_ms=4_000 + index * 37 % 60_000,
                            target={"type": "portal_display"},
                            dropped_frames=(index * 13) % 40 if index % 3 == 0 else 0)
        else:
            (directory / "capture.png").write_bytes(png)
            metadata.update(kind="screenshot", mode="display", mime_type="image/png")
        (directory / "metadata.json").write_text(json.dumps(metadata), encoding="utf-8")


def write_video_entry(root: Path, seconds=4) -> str:
    """One real 640x360 mpeg4 recording (ffmpeg testsrc2) with its
    first frame as preview.png, for the recording editor; returns its id."""
    root.mkdir(parents=True, exist_ok=True)
    entry_id = str(uuid.uuid4())
    directory = root / entry_id
    directory.mkdir()
    media = directory / "media.mp4"
    subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                    f"testsrc2=size=640x360:rate=30:duration={seconds}",
                    "-c:v", "mpeg4", "-q:v", "2", "-an", str(media)], check=True)
    subprocess.run(["ffmpeg", "-v", "error", "-i", str(media), "-frames:v", "1",
                    str(directory / "preview.png")], check=True)
    # Recordings keep their exported file outside History, as `saved_path`.
    exports = root.with_name(root.name + "-exports")
    exports.mkdir(exist_ok=True)
    saved = exports / f"Recording {entry_id[:8]}.mp4"
    saved.write_bytes(media.read_bytes())
    (directory / "metadata.json").write_text(json.dumps({
        "id": entry_id, "kind": "video", "preview_url": "", "full_url": "",
        "width": 640, "height": 360, "size_bytes": media.stat().st_size,
        "created_at": datetime.now(timezone.utc).isoformat(), "mime_type": "video/mp4",
        "duration_ms": seconds * 1000, "dropped_frames": 0, "saved_path": str(saved),
        "target": {"type": "display", "display_id": "fixture"},
    }), encoding="utf-8")
    return entry_id


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--count", type=int, help="write this many mixed entries")
    parser.add_argument("--video", action="store_true",
                        help="add one real ffmpeg recording and print its id")
    args = parser.parse_args()
    if args.video:
        print(write_video_entry(args.root))
    elif args.count is None:
        write_history(args.root)
    else:
        write_history_entries(args.root, args.count)
