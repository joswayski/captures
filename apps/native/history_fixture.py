"""Create isolated on-disk screenshot history for native UI checks (no capture access)."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import struct
import uuid
import zlib


def write_completed_settings(path: Path):
    """Existing-profile fixture; first-run behavior has its own UI smoke test."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"settings_schema_version": 5, "onboarding_completed": True,
        "output_directory": str(path.parent / "exports"), "launch_at_login": False,
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10"}))


def write_history(root: Path):
    root.mkdir(parents=True, exist_ok=False)
    width, height = 640, 360
    # Asymmetric pattern exposes stretching, orientation and blank decode failures.
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            rows.extend((220, 169, 65) if x > 440 and y < 100 else
                        (66, 122, 97) if y > 290 else (46, 67, 100))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
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


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    write_history(parser.parse_args().root)
